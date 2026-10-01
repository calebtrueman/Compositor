# Compositor for Linux — architecture

A Rust rewrite of the macOS app (which is Swift/AppKit/Metal, in `../Compositor`). The UI is
[egui](https://github.com/emilk/egui) 0.36 through eframe (glow backend, X11 and Wayland);
all pixel work is on the CPU with rayon. It reads and writes the same `.comp` projects as the
macOS app — see `../docs/project-format.md`.

## Layout

| Path | What |
| --- | --- |
| `src/doc/` | The document model. `Document` holds the canvas size and a flat, bottom-to-top `Vec<Layer>`; folders are layers with `is_group`, and children name their `parent`. `transform.rs` (`LayerTransform`, `Affine`), `selection.rs` (canvas-sized coverage mask), `history.rs` (undo by whole-document snapshots; pixels sit behind `Arc`, so snapshots are cheap), `ops.rs` (layer-stack edits). |
| `src/io/` | `comp.rs` reads/writes `.comp` packages (unknown JSON fields survive in `extra` maps). `mod.rs` imports images and exports PNG/JPEG. `psd.rs` imports Photoshop files. |
| `src/render/` | `composite(doc, region, cache)` renders any rectangle of the canvas at any scale into a premultiplied `Buffer`. `blend.rs` has Photoshop's 24 blend modes; `sample.rs` samples layers and masks through their transforms (with mipmaps when zoomed out). |
| `src/adjust/` | Adjustment layers: settings (serde, manifest-compatible), rendering, Properties UI. |
| `src/effects/` | Layer effects (stroke, shadows, glows, overlay). |
| `src/text/` | Editable text layers. |
| `src/filters/` | Image > Adjustments (destructive), Image/Canvas Size, the Filter menu. |
| `src/selection/` | Select-menu commands beyond All/Deselect/Inverse. |
| `src/tools/` | One file per tool implementing `Tool`; `Tools::new` lists them in toolbar order. |
| `src/ui/` | `canvas.rs` (tiled canvas view and pointer dispatch), `layers.rs`, `properties.rs`, `menus.rs`, `dialogs.rs`, `color.rs`, `theme.rs`. Icons are Phosphor (`crate::ui::icons::*`). |
| `src/app.rs` | The window: menu bar, tabs, panels, file dialogs, shortcuts. |
| `src/project.rs` | One open document: `doc`, `history`, `path`, `view`, the edit target (image or mask), dirty regions. |

## Conventions

- **Undo**: wrap one-shot edits in `project.edit("Name", |doc| …)`. For drags and strokes, call
  `project.begin_edit("Name")` on press and `project.finish_edit()` on release; modify
  `project.doc` in between.
- **Redraw**: `project.edit` redraws everything. Inside a stroke, call
  `project.invalidate((x0, y0, x1, y1))` with the document rectangle you changed, or
  `project.invalidate_all()`.
- **Pixels**: layer images are straight-alpha sRGB `RgbaImage`s behind `Arc`; mutate with
  `Arc::make_mut`. A layer's pixels map to the document by
  `layer.transform.pixel_to_document(w, h)`; use its `inverse()` to go from document to pixels.
  Blank layers have `image: None` — allocate a canvas-sized image (transform = full canvas)
  before painting. Masks are `MaskPixels::Uniform(v)` or `Pixels(Arc<GrayImage>)`.
- **Destructive edits** to a text or shape layer call `layer.rasterized()`.
- **Selections** limit painting, fills and filters: `doc.selection.as_ref().map(|s| s.coverage(x, y))`.
- Match the surrounding code: naming, comment density, American spelling ("color").
