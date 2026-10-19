use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ad_stream_protocol::{Rectangle, VideoCodec};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
    pub keyframe_interval: u32,
}

impl EncoderConfig {
    fn validate(&self) -> Result<(), EncoderError> {
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 {
            return Err(EncoderError::InvalidConfiguration(
                "invalid frame dimensions",
            ));
        }
        if self.framerate == 0 || self.framerate > 240 {
            return Err(EncoderError::InvalidConfiguration("invalid framerate"));
        }
        if !(128..=250_000).contains(&self.bitrate_kbps) {
            return Err(EncoderError::InvalidConfiguration("invalid bitrate"));
        }
        if self.keyframe_interval == 0 || self.keyframe_interval > 36_000 {
            return Err(EncoderError::InvalidConfiguration(
                "invalid keyframe interval",
            ));
        }
        if !matches!(self.codec, VideoCodec::H264 | VideoCodec::Av1) {
            return Err(EncoderError::UnsupportedCodec(self.codec));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncoderBackend {
    Vaapi,
    Nvenc,
    V4l2,
    Software,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncoderSelection {
    pub element: &'static str,
    pub parser: &'static str,
    pub backend: EncoderBackend,
    pub codec: VideoCodec,
}

#[derive(Clone, Debug)]
pub struct EncodedPacket {
    pub sequence: u64,
    /// Dimensions from this packet's negotiated caps, including queued frames
    /// produced before a resize request.
    pub width: u32,
    pub height: u32,
    pub codec: VideoCodec,
    pub presentation_time_ns: u64,
    pub duration_ns: u64,
    pub key_frame: bool,
    pub data: Vec<u8>,
}

pub struct EncoderPipeline {
    pipeline: gst::Pipeline,
    source: gst_app::AppSrc,
    encoder: gst::Element,
    selection: EncoderSelection,
    config: EncoderConfig,
    output: mpsc::Receiver<EncodedPacket>,
    errors: mpsc::Receiver<String>,
    next_pts: u64,
    applied_bitrate: AtomicU32,
}

impl EncoderPipeline {
    pub fn discover(codec: VideoCodec) -> Result<EncoderSelection, EncoderError> {
        discover_with(codec, |name| gst::ElementFactory::find(name).is_some())
    }

    pub fn new(config: EncoderConfig) -> Result<Self, EncoderError> {
        config.validate()?;
        gst::init()?;
        let selection = Self::discover(config.codec)?;

        let pipeline = gst::Pipeline::with_name("aetherdesk-arpp-encoder");
        let source = gst::ElementFactory::make("appsrc")
            .name("arpp-frame-source")
            .build()?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| EncoderError::ElementType("appsrc"))?;
        let queue = gst::ElementFactory::make("queue")
            .name("fresh-frame-queue")
            .property("max-size-buffers", 2_u32)
            .property_from_str("leaky", "downstream")
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        // Keep the wire stream in 8-bit 4:2:0. Without this constraint x264
        // selects 4:4:4 from BGRA, which D3D11 H.264 decoders cannot accept.
        let raw_filter = gst::ElementFactory::make("capsfilter")
            .name("arpp-encoder-input-format")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "NV12")
                    .build(),
            )
            .build()?;
        let encoder = gst::ElementFactory::make(selection.element)
            .name("adaptive-video-encoder")
            .build()?;
        let parser = gst::ElementFactory::make(selection.parser).build()?;
        if parser.find_property("config-interval").is_some() {
            parser.set_property("config-interval", -1_i32);
        }
        let stream_filter = gst::ElementFactory::make("capsfilter")
            .name("arpp-stream-format")
            .property("caps", encoded_output_caps(config.codec))
            .build()?;
        let sink = gst::ElementFactory::make("appsink")
            .name("arpp-packet-sink")
            .build()?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| EncoderError::ElementType("appsink"))?;

        source.set_is_live(true);
        source.set_block(false);
        source.set_format(gst::Format::Time);
        source.set_do_timestamp(false);
        source.set_max_bytes(u64::from(config.width) * u64::from(config.height) * 8);
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .field("width", config.width as i32)
            .field("height", config.height as i32)
            .field("framerate", gst::Fraction::new(config.framerate as i32, 1))
            .build();
        source.set_caps(Some(&caps));

        sink.set_sync(false);
        sink.set_drop(true);
        sink.set_max_buffers(2);

        configure_encoder(&encoder, &selection, &config);
        pipeline.add_many([
            source.upcast_ref(),
            &queue,
            &convert,
            &raw_filter,
            &encoder,
            &parser,
            &stream_filter,
            sink.upcast_ref(),
        ])?;
        gst::Element::link_many([
            source.upcast_ref(),
            &queue,
            &convert,
            &raw_filter,
            &encoder,
            &parser,
            &stream_filter,
            sink.upcast_ref(),
        ])?;

        let (output_tx, output) = mpsc::channel(8);
        let sequence = AtomicU64::new(1);
        let codec = config.codec;
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let caps = sample.caps().ok_or(gst::FlowError::Error)?;
                    let format = caps.structure(0).ok_or(gst::FlowError::Error)?;
                    let width = format
                        .get::<i32>("width")
                        .map_err(|_| gst::FlowError::Error)?;
                    let height = format
                        .get::<i32>("height")
                        .map_err(|_| gst::FlowError::Error)?;
                    // Older h264parse builds keep the previous frame duration
                    // on the first packet after a rate change, even though its
                    // SPS/caps already advertise the new rate. Use this sample's
                    // negotiated rate so queued packets keep their own cadence.
                    let duration_ns = format
                        .get::<gst::Fraction>("framerate")
                        .ok()
                        .filter(|rate| rate.numer() > 0 && rate.denom() > 0)
                        .map(|rate| 1_000_000_000_u64 * rate.denom() as u64 / rate.numer() as u64)
                        .or_else(|| buffer.duration().map(|time| time.nseconds()))
                        .unwrap_or(0);
                    let packet = EncodedPacket {
                        width: u32::try_from(width).map_err(|_| gst::FlowError::Error)?,
                        height: u32::try_from(height).map_err(|_| gst::FlowError::Error)?,
                        sequence: sequence.fetch_add(1, Ordering::Relaxed),
                        codec,
                        presentation_time_ns: buffer.pts().map(|time| time.nseconds()).unwrap_or(0),
                        duration_ns,
                        key_frame: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
                        data: map.as_slice().to_vec(),
                    };
                    match output_tx.try_send(packet) {
                        Ok(()) => {}
                        // Losing encoded reference frames silently corrupts later frames.
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            return Err(gst::FlowError::Error)
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            return Err(gst::FlowError::Eos);
                        }
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let (error_tx, errors) = mpsc::channel(4);
        if let Some(bus) = pipeline.bus() {
            bus.set_sync_handler(move |_, message| {
                if let gst::MessageView::Error(error) = message.view() {
                    let detail = format!(
                        "{}: {}{}",
                        error
                            .src()
                            .map(|source| source.path_string())
                            .unwrap_or_else(|| "GStreamer".into()),
                        error.error(),
                        error
                            .debug()
                            .map(|debug| format!(" ({debug})"))
                            .unwrap_or_default()
                    );
                    let _ = error_tx.try_send(detail);
                }
                // Errors are forwarded above; no async bus reader exists.
                gst::BusSyncReply::Drop
            });
        }

        pipeline.set_state(gst::State::Playing)?;
        Ok(Self {
            pipeline,
            source,
            encoder,
            selection,
            applied_bitrate: AtomicU32::new(config.bitrate_kbps),
            config,
            output,
            errors,
            next_pts: 0,
        })
    }

    pub fn selection(&self) -> &EncoderSelection {
        &self.selection
    }

    /// Renegotiate the raw input size before submitting the next captured frame.
    /// Keep the pipeline and timestamps so queued packets retain their identity.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), EncoderError> {
        if (width, height) == (self.config.width, self.config.height) {
            return Ok(());
        }
        let config = EncoderConfig {
            width,
            height,
            ..self.config.clone()
        };
        config.validate()?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(config.framerate as i32, 1))
            .build();
        self.source.set_caps(Some(&caps));
        self.source
            .set_max_bytes(u64::from(width) * u64::from(height) * 8);
        self.config = config;
        Ok(())
    }

    pub fn push_bgra_frame(
        &mut self,
        pixels: &[u8],
        stride: usize,
        _damage: &[Rectangle],
    ) -> Result<(), EncoderError> {
        let row_bytes = self.config.width as usize * 4;
        if stride < row_bytes || pixels.len() < stride * self.config.height as usize {
            return Err(EncoderError::InvalidFrame);
        }
        let packed_len = row_bytes * self.config.height as usize;
        let mut buffer = gst::Buffer::with_size(packed_len)?;
        {
            let buffer = buffer.get_mut().ok_or(EncoderError::BufferNotWritable)?;
            {
                let mut map = buffer
                    .map_writable()
                    .map_err(|_| EncoderError::BufferNotWritable)?;
                for y in 0..self.config.height as usize {
                    let source = &pixels[y * stride..y * stride + row_bytes];
                    let destination = &mut map.as_mut_slice()[y * row_bytes..(y + 1) * row_bytes];
                    destination.copy_from_slice(source);
                }
            }
            let duration = gst::ClockTime::SECOND / self.config.framerate as u64;
            buffer.set_pts(gst::ClockTime::from_nseconds(self.next_pts));
            buffer.set_duration(duration);
            self.next_pts = self.next_pts.saturating_add(duration.nseconds());
        }
        self.source.push_buffer(buffer)?;
        Ok(())
    }

    pub async fn next_packet(&mut self) -> Result<EncodedPacket, EncoderError> {
        tokio::select! {
            packet = self.output.recv() => packet.ok_or(EncoderError::OutputClosed),
            error = self.errors.recv() => Err(EncoderError::Pipeline(
                error.unwrap_or_else(|| "GStreamer error channel closed".into())
            )),
        }
    }

    pub fn pause(&self) -> Result<(), EncoderError> {
        self.pipeline.set_state(gst::State::Paused)?;
        Ok(())
    }

    pub fn resume(&self) -> Result<(), EncoderError> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    pub fn request_keyframe(&self) {
        self.encoder.send_event(
            gstreamer_video::UpstreamForceKeyUnitEvent::builder()
                .all_headers(true)
                .build(),
        );
    }

    /// Keep encoder rate control and timestamps in step with actual delivery.
    /// Advertising 30 fps while submitting 5 spends only a fraction of the
    /// intended bit budget on each frame and needlessly destroys detail.
    pub fn set_framerate(&mut self, framerate: u32) -> Result<(), EncoderError> {
        if framerate == 0 || framerate > 240 {
            return Err(EncoderError::InvalidConfiguration("invalid framerate"));
        }
        if self.config.framerate == framerate {
            return Ok(());
        }
        self.config.framerate = framerate;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRA")
            .field("width", self.config.width as i32)
            .field("height", self.config.height as i32)
            .field("framerate", gst::Fraction::new(framerate as i32, 1))
            .build();
        self.source.set_caps(Some(&caps));
        self.request_keyframe();
        Ok(())
    }

    pub fn set_bitrate(&self, bitrate_kbps: u32) {
        if self.applied_bitrate.swap(bitrate_kbps, Ordering::Relaxed) == bitrate_kbps {
            return;
        }
        for property in ["bitrate", "target-bitrate"] {
            if self.encoder.find_property(property).is_some() {
                self.encoder.set_property(property, bitrate_kbps);
                return;
            }
        }
    }
}

fn encoded_output_caps(codec: VideoCodec) -> gst::Caps {
    match codec {
        VideoCodec::H264 => gst::Caps::builder("video/x-h264")
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build(),
        VideoCodec::Av1 => gst::Caps::builder("video/x-av1")
            .field("stream-format", "obu-stream")
            .field("alignment", "frame")
            .build(),
        _ => unreachable!("encoder configuration validates the codec"),
    }
}

impl Drop for EncoderPipeline {
    fn drop(&mut self) {
        let _ = self.source.end_of_stream();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn discover_with(
    codec: VideoCodec,
    available: impl Fn(&str) -> bool,
) -> Result<EncoderSelection, EncoderError> {
    let candidates: &[EncoderSelection] = match codec {
        VideoCodec::H264 => &[
            selection("vah264lpenc", "h264parse", EncoderBackend::Vaapi, codec),
            selection("vah264enc", "h264parse", EncoderBackend::Vaapi, codec),
            selection("vaapih264enc", "h264parse", EncoderBackend::Vaapi, codec),
            selection("nvh264enc", "h264parse", EncoderBackend::Nvenc, codec),
            selection("v4l2h264enc", "h264parse", EncoderBackend::V4l2, codec),
            selection("x264enc", "h264parse", EncoderBackend::Software, codec),
            selection("openh264enc", "h264parse", EncoderBackend::Software, codec),
            selection("avenc_h264", "h264parse", EncoderBackend::Software, codec),
        ],
        VideoCodec::Av1 => &[
            selection("vaav1enc", "av1parse", EncoderBackend::Vaapi, codec),
            selection("nvav1enc", "av1parse", EncoderBackend::Nvenc, codec),
            selection("v4l2av1enc", "av1parse", EncoderBackend::V4l2, codec),
            selection("svtav1enc", "av1parse", EncoderBackend::Software, codec),
            selection("rav1enc", "av1parse", EncoderBackend::Software, codec),
            selection("av1enc", "av1parse", EncoderBackend::Software, codec),
        ],
        _ => return Err(EncoderError::UnsupportedCodec(codec)),
    };
    candidates
        .iter()
        .find(|candidate| available(candidate.element) && available(candidate.parser))
        .cloned()
        .ok_or(EncoderError::NoEncoder(codec))
}

const fn selection(
    element: &'static str,
    parser: &'static str,
    backend: EncoderBackend,
    codec: VideoCodec,
) -> EncoderSelection {
    EncoderSelection {
        element,
        parser,
        backend,
        codec,
    }
}

fn configure_encoder(encoder: &gst::Element, selection: &EncoderSelection, config: &EncoderConfig) {
    for property in ["bitrate", "target-bitrate"] {
        if encoder.find_property(property).is_some() {
            encoder.set_property(property, config.bitrate_kbps);
            break;
        }
    }
    if encoder.find_property("key-int-max").is_some() {
        encoder.set_property("key-int-max", config.keyframe_interval);
    }
    match selection.element {
        "x264enc" => {
            encoder.set_property_from_str("tune", "zerolatency");
            // Keep the lowest CPU-cost preset for interactive desktop latency.
            // Quality is bounded separately by qp-max below.
            encoder.set_property_from_str("speed-preset", "ultrafast");
            // Keep colour/detail usable during bandwidth adaptation. The
            // supervisor reduces frame rate and keeps transport queues bounded.
            encoder.set_property("qp-max", 28_u32);
            encoder.set_property("bframes", 0_u32);
            set_u32_property_if_present(encoder, "ref", 1);
            encoder.set_property("byte-stream", true);
        }
        "nvh264enc" | "nvav1enc" => {
            // p1 is the fastest NVENC preset; older plugin builds only know
            // the legacy low-latency names.
            set_string_property_if_present(encoder, "preset", &["p1", "low-latency-hq", "llhq"]);
            set_string_property_if_present(encoder, "tune", &["ultra-low-latency", "ull"]);
            set_bool_property_if_present(encoder, "zerolatency", true);
            set_bool_property_if_present(encoder, "constrained-encoding", true);
        }
        name if name.starts_with("va") => {
            set_bool_property_if_present(encoder, "aud", true);
            set_bool_property_if_present(encoder, "low-delay", true);
            set_u32_property_if_present(encoder, "b-frames", 0);
        }
        _ => {}
    }
}

/// Applies the first candidate value the element understands. Values the
/// installed plugin cannot parse (for example a preset only newer builds
/// provide) are skipped: `set_property_from_str` panics on unknown enum nicks.
fn set_string_property_if_present(element: &gst::Element, name: &str, candidates: &[&str]) {
    let Some(pspec) = element.find_property(name) else {
        return;
    };
    for value in candidates {
        if gst::glib::Value::deserialize(value, pspec.value_type()).is_ok() {
            element.set_property_from_str(name, value);
            return;
        }
    }
}

fn set_bool_property_if_present(element: &gst::Element, name: &str, value: bool) {
    if element.find_property(name).is_some() {
        element.set_property(name, value);
    }
}

fn set_u32_property_if_present(element: &gst::Element, name: &str, value: u32) {
    if element.find_property(name).is_some() {
        element.set_property(name, value);
    }
}

#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("invalid encoder configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("video codec {0:?} is unsupported by the Linux agent")]
    UnsupportedCodec(VideoCodec),
    #[error("no installed GStreamer encoder supports {0:?}")]
    NoEncoder(VideoCodec),
    #[error("GStreamer element {0} has an unexpected type")]
    ElementType(&'static str),
    #[error("the BGRA frame dimensions or stride are invalid")]
    InvalidFrame,
    #[error("the GStreamer buffer is not writable")]
    BufferNotWritable,
    #[error("the encoded packet output has closed")]
    OutputClosed,
    #[error("the GStreamer pipeline failed: {0}")]
    Pipeline(String),
    #[error("GStreamer initialization failed: {0}")]
    Init(#[from] gst::glib::Error),
    #[error("GStreamer state transition failed: {0}")]
    State(#[from] gst::StateChangeError),
    #[error("GStreamer flow failed: {0}")]
    Flow(#[from] gst::FlowError),
    #[error("GStreamer buffer allocation failed: {0}")]
    Bool(#[from] gst::glib::BoolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_encoder_limits_destructive_quantization() {
        gst::init().unwrap();
        let encoder = gst::ElementFactory::make("x264enc").build().unwrap();
        configure_encoder(
            &encoder,
            &selection(
                "x264enc",
                "h264parse",
                EncoderBackend::Software,
                VideoCodec::H264,
            ),
            &EncoderConfig {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                framerate: 30,
                bitrate_kbps: 8000,
                keyframe_interval: 30,
            },
        );
        assert!(
            encoder.property::<u32>("qp-max") <= 28,
            "the encoder may smear the desktop at maximum compression"
        );
        assert_eq!(
            encoder.property::<u32>("bframes"),
            0,
            "B-frames buffer the desktop behind future frames"
        );
        if encoder.find_property("ref").is_some() {
            assert_eq!(
                encoder.property::<u32>("ref"),
                1,
                "extra reference frames add latency"
            );
        }
    }

    #[test]
    fn unknown_encoder_values_are_skipped_instead_of_panicking() {
        gst::init().unwrap();
        let encoder = gst::ElementFactory::make("x264enc").build().unwrap();
        let before = format!("{:?}", encoder.property_value("speed-preset"));
        // A nick this plugin build does not know must leave the property alone.
        set_string_property_if_present(&encoder, "speed-preset", &["nonexistent-preset"]);
        assert_eq!(
            format!("{:?}", encoder.property_value("speed-preset")),
            before,
            "an unknown preset nick must be ignored"
        );
        // The list falls through to the first candidate the plugin understands.
        set_string_property_if_present(
            &encoder,
            "speed-preset",
            &["nonexistent-preset", "superfast"],
        );
        assert_ne!(
            format!("{:?}", encoder.property_value("speed-preset")),
            before,
            "the first known preset must be applied"
        );
    }

    #[test]
    fn hardware_precedes_software() {
        let selection = discover_with(VideoCodec::H264, |name| {
            matches!(name, "nvh264enc" | "x264enc" | "h264parse")
        })
        .unwrap();
        assert_eq!(selection.element, "nvh264enc");
        assert_eq!(selection.backend, EncoderBackend::Nvenc);
    }

    #[test]
    fn software_fallback_is_selected() {
        let selection = discover_with(VideoCodec::Av1, |name| {
            matches!(name, "svtav1enc" | "av1parse")
        })
        .unwrap();
        assert_eq!(selection.element, "svtav1enc");
        assert_eq!(selection.backend, EncoderBackend::Software);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn installed_h264_pipeline_produces_a_packet() {
        gst::init().unwrap();
        if EncoderPipeline::discover(VideoCodec::H264).is_err() {
            // Minimal build environments are allowed to omit runtime plugins;
            // production packaging installs at least one candidate.
            return;
        }
        let mut encoder = EncoderPipeline::new(EncoderConfig {
            codec: VideoCodec::H264,
            width: 64,
            height: 64,
            framerate: 30,
            bitrate_kbps: 512,
            keyframe_interval: 30,
        })
        .unwrap();
        encoder
            .push_bgra_frame(
                &vec![0_u8; 64 * 64 * 4],
                64 * 4,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                }],
            )
            .unwrap();
        let packet = tokio::time::timeout(std::time::Duration::from_secs(5), encoder.next_packet())
            .await
            .expect("encoder timed out")
            .expect("encoder returned an error");
        assert!(!packet.data.is_empty());
        assert_eq!(packet.codec, VideoCodec::H264);
        let caps = encoder
            .pipeline
            .by_name("arpp-stream-format")
            .unwrap()
            .static_pad("src")
            .unwrap()
            .current_caps()
            .unwrap();
        let profile = caps.structure(0).unwrap().get::<String>("profile").unwrap();
        assert!(
            matches!(
                profile.as_str(),
                "constrained-baseline" | "baseline" | "main" | "high"
            ),
            "H.264 profile {profile} is incompatible with the Windows hardware decoder"
        );
    }
}
