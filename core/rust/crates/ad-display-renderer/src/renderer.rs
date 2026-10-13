use ad_display_decoder::{CursorState, DecodedFrame, PresentationUpdate};
use ad_stream_protocol::Rectangle;
use thiserror::Error;

const INTERNAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

#[derive(Clone, Copy, Debug)]
pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub target_format: wgpu::TextureFormat,
    pub maximum_damage_rectangles: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererStats {
    pub submitted_frames: u64,
    pub uploaded_bytes: u64,
    pub damage_passes: u64,
}

pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    maximum_texture_dimension: u32,
    maximum_damage_rectangles: usize,
    base: wgpu::Texture,
    video: wgpu::Texture,
    cursor: wgpu::Texture,
    composed: wgpu::Texture,
    sampler: wgpu::Sampler,
    layer_layout: wgpu::BindGroupLayout,
    present_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    present_pipeline: wgpu::RenderPipeline,
    layer_bind_group: wgpu::BindGroup,
    present_bind_group: wgpu::BindGroup,
    pending_damage: Vec<Rectangle>,
    cursor_region: Option<Rectangle>,
    initialized: bool,
    stats: RendererStats,
}

impl GpuRenderer {
    pub async fn new(
        instance: &wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        config: RendererConfig,
    ) -> Result<Self, RendererError> {
        validate_dimensions(config.width, config.height)?;
        if config.maximum_damage_rectangles == 0 {
            return Err(RendererError::InvalidConfiguration);
        }
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface,
                apply_limit_buckets: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("AetherDesk ARPP renderer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await?;
        Self::from_device(device, queue, config)
    }

    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: RendererConfig,
    ) -> Result<Self, RendererError> {
        validate_dimensions(config.width, config.height)?;
        let maximum_texture_dimension = device.limits().max_texture_dimension_2d;
        if config.width > maximum_texture_dimension || config.height > maximum_texture_dimension {
            return Err(RendererError::TextureLimit {
                requested_width: config.width,
                requested_height: config.height,
                maximum: maximum_texture_dimension,
            });
        }
        let base = create_layer(&device, "ARPP base layer", config.width, config.height);
        let video = create_layer(&device, "ARPP video layer", config.width, config.height);
        let cursor = create_layer(&device, "ARPP cursor layer", config.width, config.height);
        let composed = create_composed(&device, config.width, config.height);
        clear_layers(&device, &queue, [&base, &video, &cursor]);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ARPP layer compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ARPP nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ARPP layer bind layout"),
            entries: &[
                sampler_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
            ],
        });
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ARPP presentation bind layout"),
            entries: &[sampler_entry(0), texture_entry(1)],
        });
        let base_view = base.create_view(&Default::default());
        let video_view = video.create_view(&Default::default());
        let cursor_view = cursor.create_view(&Default::default());
        let composed_view = composed.create_view(&Default::default());
        let layer_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ARPP layers"),
            layout: &layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&video_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&cursor_view),
                },
            ],
        });
        let present_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ARPP composed frame"),
            layout: &present_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&composed_view),
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ARPP composite pipeline layout"),
            bind_group_layouts: &[Some(&layer_layout)],
            immediate_size: 0,
        });
        let present_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ARPP present pipeline layout"),
                bind_group_layouts: &[None, Some(&present_layout)],
                immediate_size: 0,
            });
        let composite_pipeline = create_pipeline(
            &device,
            &shader,
            &composite_layout,
            "composite",
            INTERNAL_FORMAT,
        );
        let present_pipeline = create_pipeline(
            &device,
            &shader,
            &present_pipeline_layout,
            "present",
            config.target_format,
        );
        Ok(Self {
            device,
            queue,
            width: config.width,
            height: config.height,
            maximum_texture_dimension,
            maximum_damage_rectangles: config.maximum_damage_rectangles,
            base,
            video,
            cursor,
            composed,
            sampler,
            layer_layout,
            present_layout,
            composite_pipeline,
            present_pipeline,
            layer_bind_group,
            present_bind_group,
            pending_damage: vec![full_region(config.width, config.height)],
            cursor_region: None,
            initialized: false,
            stats: RendererStats::default(),
        })
    }

    pub fn apply(&mut self, update: &PresentationUpdate) -> Result<(), RendererError> {
        match update {
            PresentationUpdate::BaseLayer {
                width,
                height,
                pixels_bgra,
                damage,
                ..
            } => {
                self.ensure_dimensions(*width, *height)?;
                upload_damage(
                    &self.queue,
                    &self.base,
                    *width,
                    *height,
                    *width as usize * 4,
                    pixels_bgra,
                    damage,
                )?;
                self.record_damage(damage);
                self.stats.uploaded_bytes = self
                    .stats
                    .uploaded_bytes
                    .saturating_add(damaged_bytes(damage));
            }
            PresentationUpdate::BasePatch {
                region,
                stride,
                pixels_bgra,
                ..
            } => {
                validate_region(*region, self.width, self.height)?;
                upload_region(&self.queue, &self.base, *region, *stride, pixels_bgra)?;
                self.record_damage(&[*region]);
                self.stats.uploaded_bytes = self
                    .stats
                    .uploaded_bytes
                    .saturating_add(u64::from(region.width) * u64::from(region.height) * 4);
            }
            PresentationUpdate::VideoLayer(frame) => self.upload_video(frame)?,
            PresentationUpdate::Cursor(cursor) => self.upload_cursor(cursor)?,
            PresentationUpdate::CacheReset(_) => {
                self.clear_texture(&self.base);
                self.clear_texture(&self.video);
                self.clear_texture(&self.cursor);
                self.cursor_region = None;
                self.pending_damage = vec![full_region(self.width, self.height)];
            }
            PresentationUpdate::Decode { .. } | PresentationUpdate::None => {}
        }
        Ok(())
    }

    pub fn render_to_view(
        &mut self,
        target: &wgpu::TextureView,
    ) -> Result<RendererStats, RendererError> {
        let damage = normalize_damage(
            std::mem::take(&mut self.pending_damage),
            self.width,
            self.height,
            self.maximum_damage_rectangles,
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ARPP render commands"),
            });
        let composed_view = self.composed.create_view(&Default::default());
        for (index, region) in damage.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ARPP damaged layer composition"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &composed_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if !self.initialized && index == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, &self.layer_bind_group, &[]);
            pass.set_scissor_rect(region.x, region.y, region.width, region.height);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ARPP presentation"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.present_pipeline);
            pass.set_bind_group(1, &self.present_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.initialized = true;
        self.stats.submitted_frames = self.stats.submitted_frames.saturating_add(1);
        self.stats.damage_passes = self.stats.damage_passes.saturating_add(damage.len() as u64);
        Ok(self.stats)
    }

    pub fn stats(&self) -> RendererStats {
        self.stats
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        validate_dimensions(width, height)?;
        if width > self.maximum_texture_dimension || height > self.maximum_texture_dimension {
            return Err(RendererError::TextureLimit {
                requested_width: width,
                requested_height: height,
                maximum: self.maximum_texture_dimension,
            });
        }
        if width == self.width && height == self.height {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.base = create_layer(&self.device, "ARPP base layer", width, height);
        self.video = create_layer(&self.device, "ARPP video layer", width, height);
        self.cursor = create_layer(&self.device, "ARPP cursor layer", width, height);
        self.composed = create_composed(&self.device, width, height);
        clear_layers(
            &self.device,
            &self.queue,
            [&self.base, &self.video, &self.cursor],
        );
        let base_view = self.base.create_view(&Default::default());
        let video_view = self.video.create_view(&Default::default());
        let cursor_view = self.cursor.create_view(&Default::default());
        let composed_view = self.composed.create_view(&Default::default());
        self.layer_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ARPP layers"),
            layout: &self.layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&video_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&cursor_view),
                },
            ],
        });
        self.present_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ARPP composed frame"),
            layout: &self.present_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&composed_view),
                },
            ],
        });
        self.pending_damage = vec![full_region(width, height)];
        self.cursor_region = None;
        self.initialized = false;
        Ok(())
    }

    fn upload_video(&mut self, frame: &DecodedFrame) -> Result<(), RendererError> {
        self.ensure_dimensions(frame.width, frame.height)?;
        upload_damage(
            &self.queue,
            &self.video,
            frame.width,
            frame.height,
            frame.stride,
            &frame.pixels_bgra,
            &frame.damage,
        )?;
        self.record_damage(&frame.damage);
        self.stats.uploaded_bytes = self
            .stats
            .uploaded_bytes
            .saturating_add(damaged_bytes(&frame.damage));
        Ok(())
    }

    fn upload_cursor(&mut self, cursor: &CursorState) -> Result<(), RendererError> {
        if let Some(previous) = self.cursor_region.take() {
            clear_region(&self.queue, &self.cursor, previous);
            self.record_damage(&[previous]);
        }
        if !cursor.visible {
            return Ok(());
        }
        let unclipped_x = cursor.x.saturating_sub(cursor.hotspot_x as i32);
        let unclipped_y = cursor.y.saturating_sub(cursor.hotspot_y as i32);
        let source_x = unclipped_x.saturating_neg() as u32;
        let source_y = unclipped_y.saturating_neg() as u32;
        let x = unclipped_x.max(0) as u32;
        let y = unclipped_y.max(0) as u32;
        if source_x >= cursor.width || source_y >= cursor.height {
            return Ok(());
        }
        let width = (cursor.width - source_x).min(self.width.saturating_sub(x));
        let height = (cursor.height - source_y).min(self.height.saturating_sub(y));
        if width == 0 || height == 0 {
            return Ok(());
        }
        let region = Rectangle {
            x,
            y,
            width,
            height,
        };
        let source_stride = cursor.width as usize * 4;
        let source_offset = source_y as usize * source_stride + source_x as usize * 4;
        upload_region(
            &self.queue,
            &self.cursor,
            region,
            source_stride,
            &cursor.rgba[source_offset..],
        )?;
        self.cursor_region = Some(region);
        self.record_damage(&[region]);
        Ok(())
    }

    fn ensure_dimensions(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        self.resize(width, height)
    }

    fn record_damage(&mut self, damage: &[Rectangle]) {
        self.pending_damage.extend_from_slice(damage);
    }

    fn clear_texture(&self, texture: &wgpu::Texture) {
        clear_layers(&self.device, &self.queue, [texture]);
    }
}

fn create_layer(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INTERNAL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn clear_layers<'a>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    textures: impl IntoIterator<Item = &'a wgpu::Texture>,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ARPP clear layers"),
    });
    for texture in textures {
        let view = texture.create_view(&Default::default());
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ARPP clear layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    queue.submit([encoder.finish()]);
}

fn create_composed(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ARPP persistent composed frame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INTERNAL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ARPP render pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn upload_damage(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    stride: usize,
    pixels: &[u8],
    damage: &[Rectangle],
) -> Result<(), RendererError> {
    if stride < width as usize * 4 || pixels.len() < stride * height as usize {
        return Err(RendererError::InvalidPixels);
    }
    for region in damage {
        validate_region(*region, width, height)?;
        let offset = region.y as usize * stride + region.x as usize * 4;
        upload_region(queue, texture, *region, stride, &pixels[offset..])?;
    }
    Ok(())
}

fn upload_region(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    region: Rectangle,
    stride: usize,
    pixels: &[u8],
) -> Result<(), RendererError> {
    let required = stride
        .checked_mul(region.height.saturating_sub(1) as usize)
        .and_then(|prefix| prefix.checked_add(region.width as usize * 4))
        .ok_or(RendererError::InvalidPixels)?;
    if pixels.len() < required || stride > u32::MAX as usize {
        return Err(RendererError::InvalidPixels);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: region.x,
                y: region.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride as u32),
            rows_per_image: Some(region.height),
        },
        wgpu::Extent3d {
            width: region.width,
            height: region.height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

fn clear_region(queue: &wgpu::Queue, texture: &wgpu::Texture, region: Rectangle) {
    let stride = region.width as usize * 4;
    let zeroes = vec![0_u8; stride * region.height as usize];
    let _ = upload_region(queue, texture, region, stride, &zeroes);
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), RendererError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        Err(RendererError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

fn validate_region(region: Rectangle, width: u32, height: u32) -> Result<(), RendererError> {
    if region.width == 0
        || region.height == 0
        || region
            .x
            .checked_add(region.width)
            .is_none_or(|right| right > width)
        || region
            .y
            .checked_add(region.height)
            .is_none_or(|bottom| bottom > height)
    {
        Err(RendererError::InvalidDamage)
    } else {
        Ok(())
    }
}

fn normalize_damage(
    mut damage: Vec<Rectangle>,
    width: u32,
    height: u32,
    maximum: usize,
) -> Vec<Rectangle> {
    damage.retain(|region| validate_region(*region, width, height).is_ok());
    if damage.len() > maximum {
        vec![full_region(width, height)]
    } else {
        damage
    }
}

const fn full_region(width: u32, height: u32) -> Rectangle {
    Rectangle {
        x: 0,
        y: 0,
        width,
        height,
    }
}

fn damaged_bytes(damage: &[Rectangle]) -> u64 {
    damage
        .iter()
        .map(|region| u64::from(region.width) * u64::from(region.height) * 4)
        .sum()
}

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("renderer configuration is invalid")]
    InvalidConfiguration,
    #[error(
        "render target {requested_width}x{requested_height} exceeds the GPU texture limit {maximum}"
    )]
    TextureLimit {
        requested_width: u32,
        requested_height: u32,
        maximum: u32,
    },
    #[error("renderer requires resize to {width}x{height}")]
    ResizeRequired { width: u32, height: u32 },
    #[error("pixel storage is invalid")]
    InvalidPixels,
    #[error("damage region is outside the render target")]
    InvalidDamage,
    #[error("no compatible GPU adapter is available: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("GPU device creation failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excessive_damage_is_coalesced_to_a_full_frame() {
        let damage = vec![
            Rectangle {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            };
            100
        ];
        assert_eq!(
            normalize_damage(damage, 100, 100, 64),
            vec![full_region(100, 100)]
        );
    }

    #[test]
    fn invalid_damage_is_never_submitted() {
        let damage = vec![Rectangle {
            x: 99,
            y: 99,
            width: 2,
            height: 2,
        }];
        assert!(normalize_damage(damage, 100, 100, 64).is_empty());
    }

    #[test]
    fn creates_and_submits_the_gpu_compositor_pipeline() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let renderer = pollster::block_on(GpuRenderer::new(
            &instance,
            None,
            RendererConfig {
                width: 16,
                height: 16,
                target_format: INTERNAL_FORMAT,
                maximum_damage_rectangles: 16,
            },
        ));
        let Ok(mut renderer) = renderer else {
            // Headless builders without any graphics adapter still compile and
            // run all renderer-independent damage tests.
            return;
        };
        let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ARPP renderer test target"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: INTERNAL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        renderer
            .apply(&PresentationUpdate::BasePatch {
                frame_id: 1,
                region: full_region(16, 16),
                stride: 64,
                pixels_bgra: vec![255; 16 * 16 * 4],
            })
            .unwrap();
        assert_eq!(renderer.render_to_view(&view).unwrap().submitted_frames, 1);
    }
}
