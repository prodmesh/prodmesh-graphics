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
    /// What a font file says about itself: `{ family, weight }`, or null for
    /// bytes that are not a font. The name shown in a picker must be the
    /// file's own, not a string someone typed near it.
    #[wasm_bindgen(js_name = fontFacts)]
    pub fn font_facts(bytes: &[u8]) -> JsValue {
        match pmg_text::face_info(bytes) {
            Some((family, weight)) => {
                let out = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&out, &"family".into(), &family.into());
                let _ = js_sys::Reflect::set(&out, &"weight".into(), &f64::from(weight).into());
                out.into()
            }
            None => JsValue::NULL,
        }
    }

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
        /// The resting frame, for interaction: editing happens at rest, not at
        /// a moment of the In animation.
        rest: Vec<pmg_render::scene::FrameNode>,
        /// The drag in progress, if any. The GESTURE lives in React; where the
        /// geometry says the node now is lives here (ADR 0019).
        drag: Option<pmg_render::interact::Drag>,
        /// Face bytes, kept so [`Self::rearm`] can rebuild without being handed
        /// the fonts again — an edit changes the document, never the fonts.
        faces: Vec<Vec<u8>>,
        /// Image bytes by content hash. Unlike fonts these DO change mid-session
        /// — assigning a picture is an edit — so rearm takes the current set.
        blobs: std::collections::BTreeMap<String, Vec<u8>>,
    }

    fn blob_map(
        hashes: Vec<String>,
        blobs: js_sys::Array,
    ) -> std::collections::BTreeMap<String, Vec<u8>> {
        hashes
            .into_iter()
            .zip(blobs.iter().map(|v| js_sys::Uint8Array::new(&v).to_vec()))
            .collect()
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
            image_hashes: Vec<String>,
            image_blobs: js_sys::Array,
        ) -> Result<DesignSession, JsValue> {
            let faces: Vec<Vec<u8>> = fonts
                .iter()
                .map(|v| js_sys::Uint8Array::new(&v).to_vec())
                .collect();
            let blobs = blob_map(image_hashes, image_blobs);

            let armed = design::arm(&doc_json, &data_json, &faces, &blobs)
                .map_err(|e| JsValue::from_str(&e))?;
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
            let rest = design::rest_frame(&armed);
            Ok(DesignSession {
                canvas,
                armed,
                renderer,
                rest,
                drag: None,
                faces,
                blobs,
            })
        }

        /// Re-arm this session with an edited document, keeping the GPU device.
        ///
        /// Every inspector keystroke and every finished drag lands here, and
        /// paying an adapter/device request per edit — the create path's cost —
        /// made each one a visible stall. Arms FIRST and swaps only on
        /// success: a document that no longer solves leaves the old one on
        /// screen with the error as a sentence, never a blank canvas.
        #[wasm_bindgen(js_name = rearm)]
        pub fn rearm(
            &mut self,
            doc_json: String,
            data_json: String,
            image_hashes: Vec<String>,
            image_blobs: js_sys::Array,
        ) -> Result<(), JsValue> {
            // Fonts never change mid-session; images do — assigning a picture
            // is an edit, and the bytes ride in with it.
            let blobs = blob_map(image_hashes, image_blobs);
            let armed = design::arm(&doc_json, &data_json, &self.faces, &blobs)
                .map_err(|e| JsValue::from_str(&e))?;
            if (armed.instance.canvas_w, armed.instance.canvas_h)
                != (self.armed.instance.canvas_w, self.armed.instance.canvas_h)
            {
                self.canvas = design::Canvas::new(
                    self.canvas.device().clone(),
                    self.canvas.queue().clone(),
                    armed.instance.canvas_w,
                    armed.instance.canvas_h,
                )
                .map_err(|e| JsValue::from_str(&e))?;
            }
            self.renderer = pmg_render::SceneRenderer::new(
                self.canvas.device(),
                self.canvas.queue(),
                &armed.instance,
            );
            self.rest = design::rest_frame(&armed);
            self.armed = armed;
            self.blobs = blobs;
            self.drag = None;
            Ok(())
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

        // -- Interaction (ADR 0019) -----------------------------------------
        //
        // Coordinates in and out are CANVAS coordinates — design-space pixels.
        // The caller scales from client pixels; everything else is Rust's.
        // Answers come back as JSON strings: one parse on the JS side beats a
        // Reflect::set per field, and the shapes are typed in useDesign.ts.

        /// The node id under a point, or null. Structure without fill is
        /// transparent to clicks; the topmost drawn node wins.
        #[wasm_bindgen(js_name = hitTest)]
        pub fn hit_test(&self, x: f32, y: f32) -> Option<String> {
            design::hit(&self.armed, &self.rest, x, y)
        }

        /// Selection chrome for a node id: `{ id, corners, bbox, rotated }`.
        #[wasm_bindgen(js_name = handles)]
        pub fn handles(&self, id: String) -> Result<String, JsValue> {
            design::handles_json(&self.armed, &self.rest, &id).map_err(|e| JsValue::from_str(&e))
        }

        /// Start a drag. `handle` is "move" or a compass point ("n", "se", …).
        #[wasm_bindgen(js_name = beginDrag)]
        pub fn begin_drag(
            &mut self,
            id: String,
            handle: String,
            x: f32,
            y: f32,
        ) -> Result<(), JsValue> {
            self.drag = Some(
                design::begin_drag(&self.armed, &self.rest, &id, &handle, x, y)
                    .map_err(|e| JsValue::from_str(&e))?,
            );
            Ok(())
        }

        /// The current drag proposal: `{ rect, dx, dy, w, h, guides }`. The
        /// rect is the ghost React draws; nothing has been edited yet.
        #[wasm_bindgen(js_name = dragTo)]
        pub fn drag_to(&self, x: f32, y: f32, snap: bool) -> Result<String, JsValue> {
            let drag = self
                .drag
                .as_ref()
                .ok_or_else(|| JsValue::from_str("no drag"))?;
            design::drag_json(&self.armed, &self.rest, drag, x, y, snap)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Finish the drag: returns the EDITED document as JSON, or null when
        /// the drag changed nothing. The session keeps rendering the old
        /// document until the caller re-arms with the returned one.
        #[wasm_bindgen(js_name = endDrag)]
        pub fn end_drag(&mut self, x: f32, y: f32, snap: bool) -> Result<Option<String>, JsValue> {
            let Some(drag) = self.drag.take() else {
                return Ok(None);
            };
            design::end_drag(&self.armed, &self.rest, &drag, x, y, snap)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Abandon a drag without editing anything (Escape mid-drag).
        #[wasm_bindgen(js_name = cancelDrag)]
        pub fn cancel_drag(&mut self) {
            self.drag = None;
        }

        /// Move a node by a fixed amount — the arrow keys' path. Returns the
        /// edited document as JSON.
        #[wasm_bindgen(js_name = nudge)]
        pub fn nudge(&self, id: String, dx: f32, dy: f32) -> Result<String, JsValue> {
            design::nudge(&self.armed, &id, dx, dy).map_err(|e| JsValue::from_str(&e))
        }

        /// Fix a node's size from the inspector. Returns the edited document.
        #[wasm_bindgen(js_name = setSize)]
        pub fn set_size(&self, id: String, w: f32, h: f32) -> Result<String, JsValue> {
            design::set_size(&self.armed, &id, w, h).map_err(|e| JsValue::from_str(&e))
        }

        /// The authored facts for one node — fill, radius, opacity, position —
        /// as JSON. What the inspector edits, as distinct from the solved
        /// geometry `handles` reports.
        #[wasm_bindgen(js_name = facts)]
        pub fn facts(&self, id: String) -> Result<String, JsValue> {
            design::facts_json(&self.armed, &id).map_err(|e| JsValue::from_str(&e))
        }

        /// Set the fill, encoded straight RGBA in 0..1. Returns the edited
        /// document.
        #[wasm_bindgen(js_name = setFill)]
        pub fn set_fill(
            &self,
            id: String,
            r: f32,
            g: f32,
            b: f32,
            a: f32,
        ) -> Result<String, JsValue> {
            design::set_fill(&self.armed, &id, Some([r, g, b, a]))
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Remove the fill: the box becomes structural — invisible, and
        /// transparent to clicks.
        #[wasm_bindgen(js_name = clearFill)]
        pub fn clear_fill(&self, id: String) -> Result<String, JsValue> {
            design::set_fill(&self.armed, &id, None).map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = setRadius)]
        pub fn set_radius(&self, id: String, radius: f32) -> Result<String, JsValue> {
            design::set_radius(&self.armed, &id, radius).map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = setOpacity)]
        pub fn set_opacity(&self, id: String, opacity: f32) -> Result<String, JsValue> {
            design::set_opacity(&self.armed, &id, opacity).map_err(|e| JsValue::from_str(&e))
        }

        /// Pin (absolute) or unpin (relative) a node. Returns the edited
        /// document.
        #[wasm_bindgen(js_name = setPosition)]
        pub fn set_position(&self, id: String, absolute: bool) -> Result<String, JsValue> {
            design::set_position(&self.armed, &id, absolute).map_err(|e| JsValue::from_str(&e))
        }

        /// Add a box under `parent` (the root when null). Returns
        /// `{ id, doc }` as JSON, the document embedded as an object — one
        /// parse on the JS side, and the caller selects what it just made.
        #[wasm_bindgen(js_name = addBox)]
        pub fn add_box(&self, parent: Option<String>) -> Result<String, JsValue> {
            let (id, doc) = design::add_box(&self.armed, parent.as_deref())
                .map_err(|e| JsValue::from_str(&e))?;
            let doc: serde_json::Value =
                serde_json::from_str(&doc).map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_json::to_string(&serde_json::json!({ "id": id, "doc": doc }))
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Delete a node and everything inside it. The root is refused with a
        /// sentence. Returns the edited document.
        #[wasm_bindgen(js_name = deleteNode)]
        pub fn delete_node(&self, id: String) -> Result<String, JsValue> {
            design::delete_node(&self.armed, &id).map_err(|e| JsValue::from_str(&e))
        }

        /// Add a text node under `parent` (the root when null). Returns
        /// `{ id, doc }` like `addBox`, so the caller selects what it made.
        #[wasm_bindgen(js_name = addText)]
        pub fn add_text(&self, parent: Option<String>) -> Result<String, JsValue> {
            let (id, doc) = design::add_text(&self.armed, parent.as_deref())
                .map_err(|e| JsValue::from_str(&e))?;
            let doc: serde_json::Value =
                serde_json::from_str(&doc).map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_json::to_string(&serde_json::json!({ "id": id, "doc": doc }))
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Set what a text node says — a literal, `${field}` references, or
        /// both. Fields the expression references are declared as they appear.
        #[wasm_bindgen(js_name = setTextExpr)]
        pub fn set_text_expr(&self, id: String, expr: String) -> Result<String, JsValue> {
            design::set_text_expr(&self.armed, &id, &expr).map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = setFontSize)]
        pub fn set_font_size(&self, id: String, size: f32) -> Result<String, JsValue> {
            design::set_font_size(&self.armed, &id, size).map_err(|e| JsValue::from_str(&e))
        }

        /// Set family and weight, declaring the family when it is new. `asset`
        /// is the picked font file's content hash when the caller knows it —
        /// carried on the declaration so a node can stage exactly those bytes.
        #[wasm_bindgen(js_name = setFontFamily)]
        pub fn set_font_family(
            &self,
            id: String,
            family: String,
            weight: u16,
            asset: Option<String>,
        ) -> Result<String, JsValue> {
            design::set_font_family(&self.armed, &id, &family, weight, asset.as_deref())
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Text alignment within its box: "start", "center", or "end".
        #[wasm_bindgen(js_name = setTextAlign)]
        pub fn set_text_align(&self, id: String, align: String) -> Result<String, JsValue> {
            design::set_text_align(&self.armed, &id, &align).map_err(|e| JsValue::from_str(&e))
        }

        /// Move a key to a new time, carrying its value and easing.
        #[wasm_bindgen(js_name = retimeKey)]
        pub fn retime_key(
            &self,
            timeline: String,
            node_id: String,
            prop: String,
            from: f32,
            to: f32,
        ) -> Result<String, JsValue> {
            design::retime_key(&self.armed, &timeline, &node_id, &prop, from, to)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Set one spacing edge: kind "margin"|"padding", edge
        /// top|right|bottom|left, value in design px.
        #[wasm_bindgen(js_name = setSpacing)]
        pub fn set_spacing(
            &self,
            id: String,
            kind: String,
            edge: String,
            v: f32,
        ) -> Result<String, JsValue> {
            design::set_spacing(&self.armed, &id, &kind, &edge, v)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Set an outline: encoded straight RGBA plus width, drawn inside the
        /// edge so it never grows the node.
        #[wasm_bindgen(js_name = setStroke)]
        #[allow(clippy::too_many_arguments)]
        pub fn set_stroke(
            &self,
            id: String,
            r: f32,
            g: f32,
            b: f32,
            a: f32,
            width: f32,
        ) -> Result<String, JsValue> {
            design::set_stroke(&self.armed, &id, Some(([r, g, b, a], width)))
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = clearStroke)]
        pub fn clear_stroke(&self, id: String) -> Result<String, JsValue> {
            design::set_stroke(&self.armed, &id, None).map_err(|e| JsValue::from_str(&e))
        }

        /// Set a drop shadow: colour, offset, and edge softness in design px.
        #[wasm_bindgen(js_name = setShadow)]
        #[allow(clippy::too_many_arguments)]
        pub fn set_shadow(
            &self,
            id: String,
            r: f32,
            g: f32,
            b: f32,
            a: f32,
            dx: f32,
            dy: f32,
            blur: f32,
        ) -> Result<String, JsValue> {
            design::set_shadow(&self.armed, &id, Some(([r, g, b, a], dx, dy, blur)))
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = clearShadow)]
        pub fn clear_shadow(&self, id: String) -> Result<String, JsValue> {
            design::set_shadow(&self.armed, &id, None).map_err(|e| JsValue::from_str(&e))
        }

        /// Clip a node's contents to its bounds — the overflow mask.
        #[wasm_bindgen(js_name = setClip)]
        pub fn set_clip(&self, id: String, clip: bool) -> Result<String, JsValue> {
            design::set_clip(&self.armed, &id, clip).map_err(|e| JsValue::from_str(&e))
        }

        /// One axis, fixed px or content-hugging (pass null for px).
        #[wasm_bindgen(js_name = setDimension)]
        pub fn set_dimension(
            &self,
            id: String,
            axis: String,
            px: Option<f32>,
        ) -> Result<String, JsValue> {
            design::set_dimension(&self.armed, &id, &axis, px).map_err(|e| JsValue::from_str(&e))
        }

        /// Declare a field. Label and sample start as the key.
        #[wasm_bindgen(js_name = addField)]
        pub fn add_field(&self, key: String) -> Result<String, JsValue> {
            design::add_field(&self.armed, &key).map_err(|e| JsValue::from_str(&e))
        }

        /// Edit a field's label, sample, and required flag — never its key.
        #[wasm_bindgen(js_name = updateField)]
        pub fn update_field(
            &self,
            key: String,
            label: String,
            sample: Option<String>,
            required: bool,
        ) -> Result<String, JsValue> {
            design::update_field(&self.armed, &key, &label, sample.as_deref(), required)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Remove a field — refused with the using nodes named while anything
        /// still references it.
        #[wasm_bindgen(js_name = deleteField)]
        pub fn delete_field(&self, key: String) -> Result<String, JsValue> {
            design::delete_field(&self.armed, &key).map_err(|e| JsValue::from_str(&e))
        }

        /// Add an image node under `parent` (the root when null), showing the
        /// asset `hash` names. Returns `{ id, doc }` like `addBox`.
        #[wasm_bindgen(js_name = addImage)]
        pub fn add_image(&self, parent: Option<String>, hash: String) -> Result<String, JsValue> {
            let (id, doc) = design::add_image(&self.armed, parent.as_deref(), &hash)
                .map_err(|e| JsValue::from_str(&e))?;
            let doc: serde_json::Value =
                serde_json::from_str(&doc).map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_json::to_string(&serde_json::json!({ "id": id, "doc": doc }))
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Add a full-canvas, field-driven video node. Returns `{ id, doc }`;
        /// the clip field is declared on the way in.
        #[wasm_bindgen(js_name = addVideo)]
        pub fn add_video(&self, parent: Option<String>) -> Result<String, JsValue> {
            let (id, doc) = design::add_video(&self.armed, parent.as_deref())
                .map_err(|e| JsValue::from_str(&e))?;
            let doc: serde_json::Value =
                serde_json::from_str(&doc).map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_json::to_string(&serde_json::json!({ "id": id, "doc": doc }))
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Loop a clip, or hold its last frame at the end.
        #[wasm_bindgen(js_name = setLoop)]
        pub fn set_loop(&self, id: String, looping: bool) -> Result<String, JsValue> {
            design::set_loop(&self.armed, &id, looping).map_err(|e| JsValue::from_str(&e))
        }

        /// Give a node an image or swap the one it has — a box gains a
        /// background picture, an image node changes what it shows.
        #[wasm_bindgen(js_name = setImage)]
        pub fn set_image(&self, id: String, hash: String) -> Result<String, JsValue> {
            design::set_image(&self.armed, &id, &hash).map_err(|e| JsValue::from_str(&e))
        }

        /// Take a box's background image away, returning it to its fill.
        #[wasm_bindgen(js_name = clearImage)]
        pub fn clear_image(&self, id: String) -> Result<String, JsValue> {
            design::clear_image(&self.armed, &id).map_err(|e| JsValue::from_str(&e))
        }

        /// How a picture meets a box it disagrees with: "cover", "contain",
        /// or "stretch".
        #[wasm_bindgen(js_name = setFit)]
        pub fn set_fit(&self, id: String, fit: String) -> Result<String, JsValue> {
            design::set_fit(&self.armed, &id, &fit).map_err(|e| JsValue::from_str(&e))
        }

        /// Move a node to `(newParent, index)` — the layer tree's drop:
        /// reorder, reparent, and un-parent are all this one edit. Returns the
        /// edited document; cycles and the root are refused with sentences.
        #[wasm_bindgen(js_name = reparent)]
        pub fn reparent(
            &self,
            id: String,
            new_parent: String,
            index: usize,
        ) -> Result<String, JsValue> {
            design::reparent(&self.armed, &id, &new_parent, index)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Name a layer for the eye (null clears back to the id). The id
        /// underneath never changes, so timelines keep addressing it.
        #[wasm_bindgen(js_name = setName)]
        pub fn set_name(&self, id: String, name: Option<String>) -> Result<String, JsValue> {
            design::set_name(&self.armed, &id, name.as_deref()).map_err(|e| JsValue::from_str(&e))
        }

        // -- Timeline edits, each returning the edited document --------------

        #[wasm_bindgen(js_name = addTimeline)]
        pub fn add_timeline(&self, name: String, duration_ms: f32) -> Result<String, JsValue> {
            design::add_timeline(&self.armed, &name, duration_ms).map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = deleteTimeline)]
        pub fn delete_timeline(&self, name: String) -> Result<String, JsValue> {
            design::delete_timeline(&self.armed, &name).map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = setTimelineDuration)]
        pub fn set_timeline_duration(&self, name: String, ms: f32) -> Result<String, JsValue> {
            design::set_timeline_duration(&self.armed, &name, ms).map_err(|e| JsValue::from_str(&e))
        }

        /// Set a key. `ease` is serde-encoded `pmg_anim::Ease`: `"linear"`,
        /// `{"cubic":[x1,y1,x2,y2]}` or `{"steps":n}`.
        #[wasm_bindgen(js_name = setKey)]
        pub fn set_key(
            &self,
            timeline: String,
            node_id: String,
            prop: String,
            t: f32,
            v: f32,
            ease: String,
        ) -> Result<String, JsValue> {
            design::set_key(&self.armed, &timeline, &node_id, &prop, t, v, &ease)
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = deleteKey)]
        pub fn delete_key(
            &self,
            timeline: String,
            node_id: String,
            prop: String,
            t: f32,
        ) -> Result<String, JsValue> {
            design::delete_key(&self.armed, &timeline, &node_id, &prop, t)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Every track's evaluated value at a moment, as JSON
        /// `{ "node/prop": value }` — the same evaluation the pixels use.
        #[wasm_bindgen(js_name = valuesAt)]
        pub fn values_at(&self, timeline: Option<String>, t_ms: f32) -> Result<String, JsValue> {
            design::values_at_json(&self.armed, timeline.as_deref(), t_ms)
                .map_err(|e| JsValue::from_str(&e))
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
