//! `wasm-bindgen` surface for Design mode — and, today, for spike S1.
//!
//! The browser-side half of ADR 0001: run exactly the same `pmg-render` code
//! the playout node runs, on the browser's WebGPU implementation, and hand the
//! pixels back for comparison.
//!
//! # What it returns, and why raw bytes
//!
//! [`render_figure`] returns **raw RGBA8**, not a PNG or a data URL.
//!
//! That is deliberate. Encoding through a canvas would put the browser's colour
//! management in the middle of the measurement — `toDataURL` is entitled to
//! apply a colour profile, and a comparison that goes through it would be
//! measuring Chrome's colour pipeline rather than our renderer. Raw bytes out of
//! the mapped buffer are the only honest path.
//!
//! # Constraints that are the situation, not a simplification
//!
//! Single-threaded: `SharedArrayBuffer` needs COOP/COEP, which conflicts with a
//! plain-HTTP LAN appliance that deliberately has no TLS (risk R6).

pub mod design;

#[cfg(target_arch = "wasm32")]
mod browser {
    use pmg_render::parity::{Corpus, Figure, SIZE};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// One adapter and device, requested the same way for the parity corpus and
    /// for Design.
    ///
    /// The limits are the SAME downlevel limits the native node asks for. That
    /// is what makes the designer's canvas a real check: a template that needs
    /// more than a node will grant fails here, in front of the person who can
    /// change it, rather than at assignment time on a Saturday.
    pub(crate) async fn device() -> Result<(wgpu::Device, wgpu::Queue), JsValue> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("no adapter: {e}")))?;

        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pmg-wasm"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: Default::default(),
                experimental_features: Default::default(),
                trace: Default::default(),
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("no device: {e}")))
    }

    pub(crate) fn oneshot() -> (impl FnOnce(bool), impl std::future::Future<Output = bool>) {
        futures_channel_oneshot()
    }

    /// Render one figure of the parity corpus and return raw RGBA8.
    ///
    /// Async all the way down: adapter, device, and buffer mapping are all
    /// promises in the browser. Native blocks instead — that difference is
    /// exactly why `pmg-render` has no readback function of its own.
    #[wasm_bindgen]
    pub async fn render_figure(name: String) -> Result<js_sys::Uint8Array, JsValue> {
        let figure = Figure::parse(&name)
            .ok_or_else(|| JsValue::from_str(&format!("unknown figure {name}")))?;

        let (device, queue) = device().await?;

        let corpus = Corpus::new(&device, &queue);

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pmg-parity-target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: pmg_render::TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

        corpus.render(&device, &queue, figure, &view);

        let bytes_per_row = SIZE * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-parity-readback"),
            size: (bytes_per_row * SIZE) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(enc.finish()));

        let slice = buf.slice(..);
        let (tx, rx) = futures_channel_oneshot();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx(r.is_ok());
        });
        // In the browser the queue drives mapping; polling is a no-op that
        // still needs to be requested so wgpu schedules the callback.
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if !rx.await {
            return Err(JsValue::from_str("buffer map failed"));
        }

        let data = slice
            .get_mapped_range()
            .map_err(|e| JsValue::from_str(&format!("mapped range: {e}")))?
            .to_vec();
        buf.unmap();

        Ok(js_sys::Uint8Array::from(&data[..]))
    }

    /// A one-shot channel without pulling in the `futures` crate.
    fn futures_channel_oneshot() -> (impl FnOnce(bool), impl std::future::Future<Output = bool>) {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::task::{Poll, Waker};

        struct Shared {
            value: Option<bool>,
            waker: Option<Waker>,
        }
        let shared = Rc::new(RefCell::new(Shared {
            value: None,
            waker: None,
        }));
        let s_tx = shared.clone();
        let send = move |v: bool| {
            let mut b = s_tx.borrow_mut();
            b.value = Some(v);
            if let Some(w) = b.waker.take() {
                w.wake();
            }
        };
        let recv = std::future::poll_fn(move |cx| {
            let mut b = shared.borrow_mut();
            match b.value {
                Some(v) => Poll::Ready(v),
                None => {
                    b.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        });
        (send, recv)
    }
}

/// Whether this build has the browser surface compiled in.
pub const fn has_browser_surface() -> bool {
    cfg!(target_arch = "wasm32")
}

pub fn selftest() -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Design mode's JavaScript surface.
// ---------------------------------------------------------------------------

/// The `wasm-bindgen` exports Design calls.
///
/// Kept thin on purpose: everything with a decision in it lives in
/// [`design`], which compiles natively too and is therefore testable without a
/// browser. A binding layer that starts making choices is a second
/// implementation of the thing that must never disagree with the node.
#[cfg(target_arch = "wasm32")]
mod design_api {
    use crate::design;
    use wasm_bindgen::prelude::*;

    /// Compile a Template Document to canonical CBOR plus its manifest.
    ///
    /// Returns `{ template_rev, scene: Uint8Array, manifest: string }`. The
    /// `template_rev` here is **byte-identical** to the one `pmgctl scene
    /// compile` produces for the same document, because it is the same encoder
    /// over the same types (ADR 0001).
    #[wasm_bindgen]
    pub fn compile_template(doc_json: &str) -> Result<JsValue, JsValue> {
        let c = design::compile(doc_json).map_err(|e| JsValue::from_str(&e))?;
        let out = js_sys::Object::new();
        js_sys::Reflect::set(&out, &"template_rev".into(), &c.template_rev.into())?;
        js_sys::Reflect::set(
            &out,
            &"scene".into(),
            &js_sys::Uint8Array::from(&c.scene[..]).into(),
        )?;
        js_sys::Reflect::set(&out, &"manifest".into(), &c.manifest_json.into())?;
        Ok(out.into())
    }

    /// A live design session: one GPU device, one armed template.
    ///
    /// Held across frames so scrubbing a timeline does not re-solve layout or
    /// re-rasterize glyphs — the same arm/take split the node lives by, and the
    /// reason a scrub is smooth rather than a slideshow.
    #[wasm_bindgen]
    pub struct DesignSession {
        canvas: design::Canvas,
        armed: design::Armed,
        renderer: pmg_render::SceneRenderer,
    }

    #[wasm_bindgen]
    impl DesignSession {
        /// Arm a template with data and fonts.
        ///
        /// `fonts` is an array of `Uint8Array` face bytes. There is no system
        /// font database and there must not be: a template naming a system font
        /// would render differently on a designer's laptop and a booth Mac,
        /// silently.
        #[wasm_bindgen(js_name = create)]
        pub async fn create(
            doc_json: String,
            data_json: String,
            fonts: js_sys::Array,
        ) -> Result<DesignSession, JsValue> {
            let faces: Vec<Vec<u8>> = fonts
                .iter()
                .map(|v| js_sys::Uint8Array::new(&v).to_vec())
                .collect();

            let armed =
                design::arm(&doc_json, &data_json, &faces).map_err(|e| JsValue::from_str(&e))?;
            let (device, queue) = super::browser::device().await?;
            let canvas = design::Canvas::new(
                device,
                queue,
                armed.instance.canvas_w,
                armed.instance.canvas_h,
            )
            .map_err(|e| JsValue::from_str(&e))?;
            let renderer =
                pmg_render::SceneRenderer::new(canvas.device(), canvas.queue(), &armed.instance);
            Ok(DesignSession {
                canvas,
                armed,
                renderer,
            })
        }

        #[wasm_bindgen(getter)]
        pub fn width(&self) -> u32 {
            self.armed.instance.canvas_w
        }

        #[wasm_bindgen(getter)]
        pub fn height(&self) -> u32 {
            self.armed.instance.canvas_h
        }

        /// True when something on screen could not be drawn as authored — an
        /// unknown node kind, an unknown transform. It is showing AND it is
        /// wrong, and the designer must be told.
        #[wasm_bindgen(getter)]
        pub fn degraded(&self) -> bool {
            self.armed.degraded
        }

        /// Nodes `visible_if` collapsed out of the layout. This is *why* the
        /// band got shorter, and guessing is much worse than being told.
        #[wasm_bindgen(getter)]
        pub fn omitted(&self) -> js_sys::Array {
            self.armed.omitted.iter().map(JsValue::from).collect()
        }

        /// Render one frame and return raw RGBA8.
        ///
        /// Raw bytes, not a data URL: `toDataURL` may apply a colour profile,
        /// which is fine for a picture and fatal for the parity harness that
        /// shares this path.
        #[wasm_bindgen(js_name = renderFrame)]
        pub async fn render_frame(
            &self,
            timeline: Option<String>,
            t_ms: f32,
        ) -> Result<js_sys::Uint8Array, JsValue> {
            let values = design::values_at(&self.armed, timeline.as_deref(), t_ms);
            self.canvas
                .draw(&self.renderer, &self.armed.instance, &values);
            self.canvas.begin_readback();

            let slice = self.canvas.buffer().slice(..);
            let (tx, rx) = super::browser::oneshot();
            slice.map_async(wgpu::MapMode::Read, move |r| tx(r.is_ok()));
            let _ = self
                .canvas
                .device()
                .poll(wgpu::PollType::wait_indefinitely());
            if !rx.await {
                return Err(JsValue::from_str("buffer map failed"));
            }
            let data = slice
                .get_mapped_range()
                .map_err(|e| JsValue::from_str(&format!("mapped range: {e}")))?
                .to_vec();
            self.canvas.buffer().unmap();
            Ok(js_sys::Uint8Array::from(&data[..]))
        }
    }
}
