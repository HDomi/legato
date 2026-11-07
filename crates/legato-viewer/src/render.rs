//! wgpu 렌더러: I420 프레임을 세 개의 R8 텍스처로 올리고 셰이더에서 RGB 로 바꿔 그린다.
//! 원격 화면 비율을 유지하고 남는 부분은 검게 채운다 (레터박스).

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use legato_core::codec::I420Frame;
use winit::window::Window;

/// 창 안에서 원격 화면이 그려지는 영역 (물리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Viewport {
    /// 원격 해상도 비율을 유지한 채 창에 꽉 맞춘다.
    pub fn fit(win_w: u32, win_h: u32, src_w: u32, src_h: u32) -> Self {
        let (ww, wh) = (win_w.max(1) as f32, win_h.max(1) as f32);
        if src_w == 0 || src_h == 0 {
            return Self { x: 0.0, y: 0.0, w: ww, h: wh };
        }
        let scale = (ww / src_w as f32).min(wh / src_h as f32);
        let (w, h) = (src_w as f32 * scale, src_h as f32 * scale);
        Self { x: ((ww - w) / 2.0).floor(), y: ((wh - h) / 2.0).floor(), w: w.round(), h: h.round() }
    }

    /// 창 좌표를 원격 화면의 0~1 좌표로 바꾼다. 영역 밖이면 `None`.
    pub fn normalize(&self, px: f64, py: f64) -> Option<(f32, f32)> {
        let x = (px as f32 - self.x) / self.w;
        let y = (py as f32 - self.y) / self.h;
        if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) { Some((x, y)) } else { None }
    }
}

struct Planes {
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    bind: wgpu::BindGroup,
    width: u32,
    height: u32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    planes: Option<Planes>,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window).context("렌더 표면 생성 실패")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("GPU 어댑터를 찾을 수 없음")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .context("GPU 장치 생성 실패")?;

        let caps = surface.get_capabilities(&adapter);
        // 셰이더 결과가 이미 감마 보정된 값이라 sRGB 가 아닌 형식을 고른다 (색이 바래지 않게).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow!("지원하는 표면 형식이 없음"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            // 지연을 줄이려고 Mailbox 가 되면 쓰고, 아니면 Fifo.
            present_mode: if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            },
            desired_maximum_frame_latency: 1,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Ok(Self { surface, device, queue, config, pipeline, layout, sampler, planes: None })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn make_planes(&self, width: u32, height: u32) -> Planes {
        let tex = |w, h, label| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let y = tex(width, height, "y");
        let u = tex(width / 2, height / 2, "u");
        let v = tex(width / 2, height / 2, "v");
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view(&y)) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view(&u)) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view(&v)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Planes { y, u, v, bind, width, height }
    }

    /// 새 프레임을 텍스처에 올린다. 해상도가 바뀌면 텍스처를 다시 만든다.
    pub fn upload(&mut self, f: &I420Frame) {
        let (w, h) = (f.width as u32, f.height as u32);
        if self.planes.as_ref().is_none_or(|p| p.width != w || p.height != h) {
            self.planes = Some(self.make_planes(w, h));
        }
        let p = self.planes.as_ref().expect("위에서 만듦");
        let write = |tex: &wgpu::Texture, data: &[u8], tw: u32, th: u32| {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(tw), rows_per_image: Some(th) },
                wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
            );
        };
        write(&p.y, &f.y, w, h);
        write(&p.u, &f.u, w / 2, h / 2);
        write(&p.v, &f.v, w / 2, h / 2);
    }

    /// 현재 텍스처를 그린다. 그린 영역을 돌려준다.
    pub fn render(&mut self) -> Option<Viewport> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return None;
            }
            _ => return None,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let vp = self.planes.as_ref().map(|p| Viewport::fit(self.config.width, self.config.height, p.width, p.height));
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
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
            if let (Some(p), Some(vp)) = (&self.planes, vp) {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &p.bind, &[]);
                pass.set_viewport(vp.x, vp.y, vp.w, vp.h, 0.0, 1.0);
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        vp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 레터박스_맞춤() {
        // 16:10 원격을 16:9 창에: 좌우에 여백.
        let vp = Viewport::fit(1920, 1080, 2560, 1600);
        assert_eq!(vp.h, 1080.0);
        assert_eq!(vp.w, 1728.0);
        assert_eq!(vp.x, 96.0);
        assert_eq!(vp.normalize(96.0, 0.0), Some((0.0, 0.0)));
        assert_eq!(vp.normalize(10.0, 10.0), None);
        let (x, y) = vp.normalize(96.0 + 864.0, 540.0).unwrap();
        assert!((x - 0.5).abs() < 1e-6 && (y - 0.5).abs() < 1e-6);
    }
}
