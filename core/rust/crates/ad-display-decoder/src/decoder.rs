use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ad_qos_controller::TimedFrame;
use ad_stream_protocol::{DisplayMessage, Rectangle, VideoCodec};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use thiserror::Error;
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
struct LatestFrame {
    value: Mutex<Option<DecodedFrame>>,
    ready: Notify,
}

impl LatestFrame {
    fn publish(&self, frame: DecodedFrame) {
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
        self.ready.notify_one();
    }

    async fn receive(&self) -> DecodedFrame {
        loop {
            if let Some(frame) = self.value.lock().unwrap_or_else(|e| e.into_inner()).take() {
                return frame;
            }
            self.ready.notified().await;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecoderBackend {
    D3d11,
    Vaapi,
    Nvdec,
    V4l2,
    Software,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecoderSelection {
    pub element: &'static str,
    pub parser: &'static str,
    pub backend: DecoderBackend,
    pub codec: VideoCodec,
}

#[derive(Clone, Debug)]
pub struct DecoderConfig {
    pub codec: VideoCodec,
    pub maximum_width: u32,
    pub maximum_height: u32,
    pub output_queue: usize,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            maximum_width: 16_384,
            maximum_height: 16_384,
            output_queue: 3,
        }
    }
}

impl DecoderConfig {
    fn validate(&self) -> Result<(), DecoderError> {
        if self.maximum_width == 0
            || self.maximum_height == 0
            || self.maximum_width > 16_384
            || self.maximum_height > 16_384
            || self.output_queue == 0
        {
            return Err(DecoderError::InvalidConfiguration);
        }
        if !matches!(self.codec, VideoCodec::H264 | VideoCodec::Av1) {
            return Err(DecoderError::UnsupportedCodec(self.codec));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct DecodedFrame {
    pub frame_id: u64,
    pub presentation_time_us: u64,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels_bgra: Vec<u8>,
    pub damage: Vec<Rectangle>,
}

impl TimedFrame for DecodedFrame {
    fn sequence(&self) -> u64 {
        self.frame_id
    }
    fn presentation_time_us(&self) -> u64 {
        self.presentation_time_us
    }
}

struct PendingFrame {
    presentation_time_us: u64,
    damage: Vec<Rectangle>,
}

pub struct VideoDecoder {
    pipeline: gst::Pipeline,
    source: gst_app::AppSrc,
    selection: DecoderSelection,
    checked_dimensions: Option<(u32, u32)>,
    config: DecoderConfig,
    pending: Arc<Mutex<BTreeMap<u64, PendingFrame>>>,
    output: Arc<LatestFrame>,
    errors: mpsc::Receiver<String>,
}

impl VideoDecoder {
    pub fn discover(codec: VideoCodec) -> Result<DecoderSelection, DecoderError> {
        gst::init()?;
        discover_with(codec, |name| gst::ElementFactory::find(name).is_some())
    }

    pub fn new(config: DecoderConfig) -> Result<Self, DecoderError> {
        config.validate()?;
        gst::init()?;
        let selection = Self::discover(config.codec)?;
        Self::new_with_selection(config, selection)
    }

    fn new_with_selection(
        config: DecoderConfig,
        selection: DecoderSelection,
    ) -> Result<Self, DecoderError> {
        let pipeline = gst::Pipeline::with_name("aetherdesk-arpp-decoder");
        let source = gst::ElementFactory::make("appsrc")
            .name("arpp-video-source")
            .build()?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| DecoderError::ElementType("appsrc"))?;
        let parser = gst::ElementFactory::make(selection.parser).build()?;
        let decoder = gst::ElementFactory::make(selection.element).build()?;
        if selection.element == "avdec_h264" {
            // Slice workers can parallelize one update without waiting for a
            // future frame. Frame threading is unsuitable for idle desktops.
            decoder.set_property_from_str("thread-type", "slice");
        }
        let queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 2_u32)
            .property_from_str("leaky", "downstream")
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let filter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "BGRA")
                    .build(),
            )
            .build()?;
        let sink = gst::ElementFactory::make("appsink")
            .name("arpp-decoded-frame-sink")
            .build()?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| DecoderError::ElementType("appsink"))?;

        source.set_is_live(true);
        source.set_format(gst::Format::Time);
        source.set_block(false);
        source.set_caps(Some(&encoded_caps(config.codec)));
        sink.set_sync(false);
        sink.set_drop(true);
        sink.set_max_buffers(config.output_queue as u32);

        pipeline.add_many([
            source.upcast_ref(),
            &parser,
            &decoder,
            &queue,
            &convert,
            &filter,
            sink.upcast_ref(),
        ])?;
        gst::Element::link_many([
            source.upcast_ref(),
            &parser,
            &decoder,
            &queue,
            &convert,
            &filter,
            sink.upcast_ref(),
        ])?;

        let pending = Arc::new(Mutex::new(BTreeMap::<u64, PendingFrame>::new()));
        let callback_pending = pending.clone();
        let output = Arc::new(LatestFrame::default());
        let output_tx = output.clone();
        let maximum_width = config.maximum_width;
        let maximum_height = config.maximum_height;
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let caps = sample.caps().ok_or(gst::FlowError::NotNegotiated)?;
                    let info = gst_video::VideoInfo::from_caps(caps)
                        .map_err(|_| gst::FlowError::NotNegotiated)?;
                    if info.width() > maximum_width
                        || info.height() > maximum_height
                        || u64::from(info.width()) * u64::from(info.height()) > 33_554_432
                    {
                        return Err(gst::FlowError::Error);
                    }
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let frame_id = buffer.pts().map(|pts| pts.nseconds()).unwrap_or(0);
                    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                        .map_err(|_| gst::FlowError::Error)?;
                    let source = frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
                    let source_stride = info.stride()[0].unsigned_abs() as usize;
                    let row_bytes = info.width() as usize * 4;
                    if source_stride < row_bytes
                        || source.len() < source_stride * info.height() as usize
                    {
                        return Err(gst::FlowError::Error);
                    }
                    let mut pixels = vec![0_u8; row_bytes * info.height() as usize];
                    for row in 0..info.height() as usize {
                        pixels[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(
                            &source[row * source_stride..row * source_stride + row_bytes],
                        );
                    }
                    let metadata = callback_pending
                        .lock()
                        .map_err(|_| gst::FlowError::Error)?
                        .remove(&frame_id)
                        .unwrap_or(PendingFrame {
                            presentation_time_us: 0,
                            damage: vec![Rectangle {
                                x: 0,
                                y: 0,
                                width: info.width(),
                                height: info.height(),
                            }],
                        });
                    let decoded = DecodedFrame {
                        frame_id,
                        presentation_time_us: metadata.presentation_time_us,
                        width: info.width(),
                        height: info.height(),
                        stride: row_bytes,
                        pixels_bgra: pixels,
                        damage: metadata.damage,
                    };
                    // Raw full frames are independent; replace stale output.
                    // Never discard encoded reference frames in this way.
                    output_tx.publish(decoded);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let (error_tx, errors) = mpsc::channel(4);
        if let Some(bus) = pipeline.bus() {
            bus.set_sync_handler(move |_, message| {
                if let gst::MessageView::Error(error) = message.view() {
                    let _ = error_tx.try_send(format!("{}", error.error()));
                }
                // There is no asynchronous bus consumer to drain these messages.
                gst::BusSyncReply::Drop
            });
        }
        pipeline.set_state(gst::State::Playing)?;
        Ok(Self {
            pipeline,
            source,
            selection,
            checked_dimensions: None,
            config,
            pending,
            output,
            errors,
        })
    }

    pub fn selection(&self) -> &DecoderSelection {
        &self.selection
    }

    /// Keep wide multi-monitor desktops within the selected device's advertised
    /// limits. The first frame after resizing contains fresh stream headers.
    pub fn prepare_dimensions(&mut self, width: u32, height: u32) -> Result<(), DecoderError> {
        if width == 0
            || height == 0
            || width > self.config.maximum_width
            || height > self.config.maximum_height
            || u64::from(width) * u64::from(height) > 33_554_432
        {
            return Err(DecoderError::InvalidDimensions);
        }
        if self.checked_dimensions == Some((width, height)) {
            return Ok(());
        }
        if self.selection.backend != DecoderBackend::Software {
            let supported =
                gst::ElementFactory::find(self.selection.element).is_some_and(|factory| {
                    factory.static_pad_templates().iter().any(|pad| {
                        pad.direction() == gst::PadDirection::Sink
                            && caps_support_dimensions(
                                &pad.caps(),
                                self.config.codec,
                                width,
                                height,
                            )
                    })
                });
            if !supported {
                let software = discover_with(self.config.codec, |name| {
                    matches!(
                        name,
                        "avdec_h264"
                            | "openh264dec"
                            | "dav1ddec"
                            | "av1dec"
                            | "h264parse"
                            | "av1parse"
                    ) && gst::ElementFactory::find(name).is_some()
                })?;
                *self = Self::new_with_selection(self.config.clone(), software)?;
            }
        }
        self.checked_dimensions = Some((width, height));
        Ok(())
    }

    pub async fn submit(
        &mut self,
        message: &DisplayMessage,
        damage: Vec<Rectangle>,
    ) -> Result<(), DecoderError> {
        let DisplayMessage::EncodedFrame {
            frame_id,
            presentation_time_us,
            codec,
            width,
            height,
            data,
            ..
        } = message
        else {
            return Err(DecoderError::NotEncodedFrame);
        };
        if *codec != self.config.codec {
            return Err(DecoderError::CodecChanged {
                expected: self.config.codec,
                actual: *codec,
            });
        }
        if *width == 0
            || *height == 0
            || *width > self.config.maximum_width
            || *height > self.config.maximum_height
            || u64::from(*width) * u64::from(*height) > 33_554_432
        {
            return Err(DecoderError::InvalidDimensions);
        }
        // appsrc's limits are only advisory with block=false. Admit one queued
        // access unit plus the one decoding, without blocking input tasks.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while self.source.current_level_bytes() != 0 {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .map_err(|_| DecoderError::Pipeline("decoder stopped consuming frames".into()))?;
        let mut buffer = gst::Buffer::from_mut_slice(data.clone());
        buffer
            .get_mut()
            .ok_or(DecoderError::BufferNotWritable)?
            .set_pts(gst::ClockTime::from_nseconds(*frame_id));
        let mut pending = self.pending.lock().map_err(|_| DecoderError::Poisoned)?;
        while pending.len() >= self.config.output_queue.saturating_mul(4) {
            pending.pop_first();
        }
        pending.insert(
            *frame_id,
            PendingFrame {
                presentation_time_us: *presentation_time_us,
                damage,
            },
        );
        drop(pending);
        if let Err(error) = self.source.push_buffer(buffer) {
            self.pending
                .lock()
                .map_err(|_| DecoderError::Poisoned)?
                .remove(frame_id);
            return Err(error.into());
        }
        Ok(())
    }

    pub async fn next_frame(&mut self) -> Result<DecodedFrame, DecoderError> {
        tokio::select! {
            frame = self.output.receive() => Ok(frame),
            error = self.errors.recv() => Err(DecoderError::Pipeline(
                error.unwrap_or_else(|| "decoder error channel closed".into())
            )),
        }
    }

    pub fn flush(&mut self) -> Result<(), DecoderError> {
        self.pipeline.set_state(gst::State::Ready)?;
        self.pending
            .lock()
            .map_err(|_| DecoderError::Poisoned)?
            .clear();
        self.output
            .value
            .lock()
            .map_err(|_| DecoderError::Poisoned)?
            .take();
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        let _ = self.source.end_of_stream();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn encoded_caps(codec: VideoCodec) -> gst::Caps {
    match codec {
        VideoCodec::H264 => gst::Caps::builder("video/x-h264")
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build(),
        VideoCodec::Av1 => gst::Caps::builder("video/x-av1")
            .field("stream-format", "obu-stream")
            .field("alignment", "frame")
            .build(),
        _ => unreachable!("validated before caps construction"),
    }
}

fn caps_support_dimensions(
    caps: &gst::CapsRef,
    codec: VideoCodec,
    width: u32,
    height: u32,
) -> bool {
    let media = match codec {
        VideoCodec::H264 => "video/x-h264",
        VideoCodec::Av1 => "video/x-av1",
        _ => return false,
    };
    caps.can_intersect(
        &gst::Caps::builder(media)
            .field("width", width as i32)
            .field("height", height as i32)
            .build(),
    )
}

fn discover_with(
    codec: VideoCodec,
    available: impl Fn(&str) -> bool,
) -> Result<DecoderSelection, DecoderError> {
    let candidates: &[DecoderSelection] = match codec {
        VideoCodec::H264 => &[
            // D3D11 in the packaged Windows runtime retains one H.264 picture
            // until the next access unit, even in live/low-latency mode. Our
            // damage-driven stream can stay idle indefinitely after a key or
            // click, so prefer the bundled decoder that emits isolated updates.
            #[cfg(target_os = "windows")]
            selection("avdec_h264", "h264parse", DecoderBackend::Software, codec),
            #[cfg(target_os = "windows")]
            selection("openh264dec", "h264parse", DecoderBackend::Software, codec),
            selection("d3d11h264dec", "h264parse", DecoderBackend::D3d11, codec),
            selection("nvh264dec", "h264parse", DecoderBackend::Nvdec, codec),
            selection("vah264dec", "h264parse", DecoderBackend::Vaapi, codec),
            selection("vaapih264dec", "h264parse", DecoderBackend::Vaapi, codec),
            selection("v4l2slh264dec", "h264parse", DecoderBackend::V4l2, codec),
            #[cfg(not(target_os = "windows"))]
            selection("avdec_h264", "h264parse", DecoderBackend::Software, codec),
            #[cfg(not(target_os = "windows"))]
            selection("openh264dec", "h264parse", DecoderBackend::Software, codec),
        ],
        VideoCodec::Av1 => &[
            selection("d3d11av1dec", "av1parse", DecoderBackend::D3d11, codec),
            selection("nvav1dec", "av1parse", DecoderBackend::Nvdec, codec),
            selection("vaav1dec", "av1parse", DecoderBackend::Vaapi, codec),
            selection("v4l2slav1dec", "av1parse", DecoderBackend::V4l2, codec),
            selection("dav1ddec", "av1parse", DecoderBackend::Software, codec),
            selection("av1dec", "av1parse", DecoderBackend::Software, codec),
        ],
        _ => return Err(DecoderError::UnsupportedCodec(codec)),
    };
    candidates
        .iter()
        .find(|candidate| available(candidate.element) && available(candidate.parser))
        .cloned()
        .ok_or(DecoderError::NoDecoder(codec))
}

const fn selection(
    element: &'static str,
    parser: &'static str,
    backend: DecoderBackend,
    codec: VideoCodec,
) -> DecoderSelection {
    DecoderSelection {
        element,
        parser,
        backend,
        codec,
    }
}

#[derive(Debug, Error)]
pub enum DecoderError {
    #[error("invalid decoder configuration")]
    InvalidConfiguration,
    #[error("unsupported video codec {0:?}")]
    UnsupportedCodec(VideoCodec),
    #[error("no decoder is installed for {0:?}")]
    NoDecoder(VideoCodec),
    #[error("display message is not an encoded frame")]
    NotEncodedFrame,
    #[error("encoded frame dimensions are invalid")]
    InvalidDimensions,
    #[error("video codec changed without renegotiation: expected {expected:?}, got {actual:?}")]
    CodecChanged {
        expected: VideoCodec,
        actual: VideoCodec,
    },
    #[error("decoder metadata lock was poisoned")]
    Poisoned,
    #[error("GStreamer element {0} has an unexpected type")]
    ElementType(&'static str),
    #[error("GStreamer buffer is not writable")]
    BufferNotWritable,
    #[error("decoded frame output closed")]
    OutputClosed,
    #[error("decoder pipeline failed: {0}")]
    Pipeline(String),
    #[error("GStreamer initialization failed: {0}")]
    Init(#[from] gst::glib::Error),
    #[error("GStreamer operation failed: {0}")]
    Bool(#[from] gst::glib::BoolError),
    #[error("GStreamer state transition failed: {0}")]
    State(#[from] gst::StateChangeError),
    #[error("GStreamer flow failed: {0}")]
    Flow(#[from] gst::FlowError),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use agent_encoder::{EncoderConfig, EncoderPipeline};

    #[test]
    fn wide_desktops_do_not_fit_a_four_k_hardware_decoder() {
        gst::init().unwrap();
        let caps: gst::Caps = "video/x-h264,width=(int)[16,4096],height=(int)[16,4096]"
            .parse()
            .unwrap();
        assert!(caps_support_dimensions(&caps, VideoCodec::H264, 3840, 1080));
        assert!(!caps_support_dimensions(
            &caps,
            VideoCodec::H264,
            11520,
            2160
        ));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn hardware_decoders_precede_software() {
        let selected = discover_with(VideoCodec::H264, |name| {
            matches!(name, "nvh264dec" | "avdec_h264" | "h264parse")
        })
        .unwrap();
        assert_eq!(selected.backend, DecoderBackend::Nvdec);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_h264_prefers_isolated_updates_over_hardware_throughput() {
        let selected = discover_with(VideoCodec::H264, |name| {
            matches!(
                name,
                "d3d11h264dec" | "nvh264dec" | "avdec_h264" | "h264parse"
            )
        })
        .unwrap();
        assert_eq!(selected.element, "avdec_h264");
        assert_eq!(selected.backend, DecoderBackend::Software);
    }

    #[test]
    fn software_av1_fallback_is_supported() {
        let selected = discover_with(VideoCodec::Av1, |name| {
            matches!(name, "dav1ddec" | "av1parse")
        })
        .unwrap();
        assert_eq!(selected.backend, DecoderBackend::Software);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn decodes_real_h264_across_growing_and_shrinking_resizes() {
        if VideoDecoder::discover(VideoCodec::H264).is_err()
            || EncoderPipeline::discover(VideoCodec::H264).is_err()
        {
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
        let mut decoder = VideoDecoder::new(DecoderConfig {
            codec: VideoCodec::H264,
            maximum_width: 16_384,
            maximum_height: 256,
            output_queue: 2,
        })
        .unwrap();
        let mut presentation =
            crate::RemotePresentation::new(64, 64, crate::CacheLimits::default()).unwrap();
        let mut framebuffer = crate::Framebuffer::new(64, 64).unwrap();
        let mut previous_sequence = 0;
        for (width, height) in [(64, 64), (128, 96), (11520, 64), (32, 48), (64, 64)] {
            encoder.resize(width, height).unwrap();
            encoder
                .push_bgra_frame(
                    &vec![64_u8; (width * height * 4) as usize],
                    (width * 4) as usize,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }],
                )
                .unwrap();
            let packet =
                tokio::time::timeout(std::time::Duration::from_secs(5), encoder.next_packet())
                    .await
                    .expect("encoder timed out")
                    .unwrap();
            assert_eq!((packet.width, packet.height), (width, height));
            assert!(packet.sequence > previous_sequence);
            previous_sequence = packet.sequence;
            let message = DisplayMessage::EncodedFrame {
                frame_id: packet.sequence,
                presentation_time_us: packet.presentation_time_ns / 1000,
                codec: packet.codec,
                key_frame: packet.key_frame,
                width: packet.width,
                height: packet.height,
                data: packet.data,
            };
            let crate::PresentationUpdate::Decode { message, damage } =
                presentation.handle(message).unwrap()
            else {
                panic!("expected an encoded frame");
            };
            decoder.prepare_dimensions(width, height).unwrap();
            decoder.submit(&message, damage).await.unwrap();
            let frame =
                tokio::time::timeout(std::time::Duration::from_secs(5), decoder.next_frame())
                    .await
                    .expect("decoder timed out")
                    .unwrap();
            assert_eq!(
                (frame.frame_id, frame.width, frame.height),
                (packet.sequence, width, height)
            );
            assert_eq!(frame.pixels_bgra.len(), (width * height * 4) as usize);
            let update = presentation.accept_decoded(frame).unwrap();
            assert!(framebuffer.apply(&update).unwrap());
            let (actual_width, actual_height, _) = framebuffer.snapshot();
            assert_eq!((actual_width, actual_height), (width, height));
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn moving_colour_frames_survive_the_real_video_pipeline() {
        let (width, height) = (962_u32, 542_u32); // Deliberately not macroblock-aligned.
        let mut encoder = EncoderPipeline::new(EncoderConfig {
            codec: VideoCodec::H264,
            width,
            height,
            framerate: 30,
            bitrate_kbps: 8000,
            keyframe_interval: 30,
        })
        .unwrap();
        let mut decoder = VideoDecoder::new_with_selection(
            DecoderConfig::default(),
            selection(
                "avdec_h264",
                "h264parse",
                DecoderBackend::Software,
                VideoCodec::H264,
            ),
        )
        .unwrap();
        let mut last_pts = None;
        for phase in 0..90_u32 {
            // Exercise sustained motion, congestion, and recovery on one
            // live stream. Lowering delivery cadence must not leave the
            // encoder spending a 30-fps budget on each 5-fps frame.
            let (bitrate, fps) = if (15..75).contains(&phase) {
                (666, 5)
            } else {
                (8000, 30)
            };
            encoder.set_bitrate(bitrate);
            encoder.set_framerate(fps).unwrap();
            let mut pixels = vec![255_u8; (width * height * 4) as usize];
            for y in 0..height {
                for x in 0..width {
                    let offset = ((y * width + x) * 4) as usize;
                    pixels[offset] = ((x / 4 + y / 4 + phase * 11) % 256) as u8;
                    pixels[offset + 1] = ((y / 2 + phase * 13) % 256) as u8;
                    pixels[offset + 2] = ((x / 2 + phase * 17) % 256) as u8;
                    // Fine moving luminance texture makes this exercise the
                    // compression budget, not just easy solid colour bands.
                    let hash = (x + y * width + phase * width * height).wrapping_mul(2_654_435_761);
                    let texture = ((hash >> 24) & 63) as i16 - 32;
                    for channel in &mut pixels[offset..offset + 3] {
                        *channel = (i16::from(*channel) + texture).clamp(0, 255) as u8;
                    }
                }
            }
            encoder
                .push_bgra_frame(&pixels, width as usize * 4, &[])
                .unwrap();
            let packet =
                tokio::time::timeout(std::time::Duration::from_secs(3), encoder.next_packet())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(packet.duration_ns, 1_000_000_000 / u64::from(fps));
            if let Some(previous) = last_pts {
                assert!(packet.presentation_time_ns > previous);
            }
            last_pts = Some(packet.presentation_time_ns);
            if phase == 15 || phase == 75 {
                assert!(
                    packet.key_frame,
                    "rate changes must restart with a complete image"
                );
            }
            let message = DisplayMessage::EncodedFrame {
                frame_id: packet.sequence,
                presentation_time_us: packet.presentation_time_ns / 1000,
                codec: packet.codec,
                key_frame: packet.key_frame,
                width,
                height,
                data: packet.data,
            };
            decoder.prepare_dimensions(width, height).unwrap();
            decoder.submit(&message, vec![]).await.unwrap();
            let frame =
                tokio::time::timeout(std::time::Duration::from_secs(1), decoder.next_frame())
                    .await
                    .unwrap()
                    .unwrap();
            let error: u64 = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(frame.pixels_bgra.as_chunks::<4>().0)
                .map(|(a, b)| (0..3).map(|c| u64::from(a[c].abs_diff(b[c]))).sum::<u64>())
                .sum();
            let mean_error = error as f64 / f64::from(width * height * 3);
            if phase % 15 == 0 || mean_error >= 12.0 {
                eprintln!("moving frame {phase}, {bitrate} kb/s at {fps} fps: mean colour error {mean_error:.2}/255");
            }
            assert!(
                mean_error < 12.0,
                "colours were corrupted in the video pipeline: {mean_error}"
            );
        }
    }

    // Each access unit is a complete update. No following frame or EOS may
    // be needed to display it: an idle desktop might never send another one.
    async fn assert_isolated_h264_updates(mut decoder: VideoDecoder) {
        let packets: [&[u8]; 4] = [
            include_bytes!("../tests/fixtures/frame-0.h264"),
            include_bytes!("../tests/fixtures/frame-1.h264"),
            include_bytes!("../tests/fixtures/frame-2.h264"),
            include_bytes!("../tests/fixtures/frame-3.h264"),
        ];
        for epoch in 0..2 {
            if epoch != 0 {
                decoder.flush().unwrap();
            }
            for (index, data) in packets.iter().enumerate() {
                let frame_id = epoch * 100 + index as u64 + 1;
                let presentation_time_us = frame_id * 33_333;
                let damage = vec![Rectangle {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                }];
                let message = DisplayMessage::EncodedFrame {
                    frame_id,
                    presentation_time_us,
                    codec: VideoCodec::H264,
                    key_frame: index == 0,
                    width: 64,
                    height: 64,
                    data: data.to_vec(),
                };
                decoder.prepare_dimensions(64, 64).unwrap();
                decoder.submit(&message, damage.clone()).await.unwrap();
                let started = std::time::Instant::now();
                let frame =
                    tokio::time::timeout(std::time::Duration::from_secs(1), decoder.next_frame())
                        .await
                        .expect("isolated desktop update waited for a following frame")
                        .unwrap();
                eprintln!(
                    "{} frame {frame_id}: {:?}",
                    decoder.selection().element,
                    started.elapsed()
                );
                assert_eq!(frame.frame_id, frame_id);
                assert_eq!(frame.presentation_time_us, presentation_time_us);
                assert_eq!((frame.width, frame.height, frame.stride), (64, 64, 256));
                assert_eq!(frame.pixels_bgra.len(), 64 * 64 * 4);
                assert_eq!(frame.damage, damage);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_h264_displays_each_isolated_update_without_next_frame() {
        assert_isolated_h264_updates(VideoDecoder::new(DecoderConfig::default()).unwrap()).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn software_h264_displays_each_isolated_update_without_next_frame() {
        gst::init().unwrap();
        let decoder = VideoDecoder::new_with_selection(
            DecoderConfig::default(),
            selection(
                "avdec_h264",
                "h264parse",
                DecoderBackend::Software,
                VideoCodec::H264,
            ),
        )
        .unwrap();
        assert_isolated_h264_updates(decoder).await;
    }
}

#[cfg(test)]
mod latest_tests {
    use super::*;

    #[tokio::test]
    async fn slow_consumer_gets_newest_complete_frame() {
        let output = LatestFrame::default();
        for frame_id in 1..=100 {
            output.publish(DecodedFrame {
                frame_id,
                presentation_time_us: frame_id,
                width: 1,
                height: 1,
                stride: 4,
                pixels_bgra: vec![frame_id as u8; 4],
                damage: vec![],
            });
        }
        let frame = output.receive().await;
        assert_eq!(frame.frame_id, 100);
        assert_eq!(frame.pixels_bgra, [100; 4]);
        assert!(output.value.lock().unwrap().is_none());
    }
}
