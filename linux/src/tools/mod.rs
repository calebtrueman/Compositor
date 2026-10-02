//! Canvas tools. Each tool is a `Tool` in its own file; `Tools::new` lists them in toolbar order.

pub mod blur;
pub mod brush;
pub mod clone_stamp;
pub mod crop;
pub mod eraser;
pub mod eyedropper;
pub mod gradient;
pub mod hand;
pub mod lasso;
pub mod magic_wand;
pub mod marquee;
pub mod move_tool;
pub mod paint;
pub mod paint_bucket;
pub mod shape;
pub mod spot_healing;
pub mod type_tool;

use egui::{Color32, CursorIcon, Key, Modifiers, Painter};

use crate::project::Project;
use crate::render::RenderCache;
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Move,
    Marquee,
    Lasso,
    MagicWand,
    Crop,
    Eyedropper,
    SpotHealing,
    Brush,
    Eraser,
    CloneStamp,
    Blur,
    Gradient,
    PaintBucket,
    Shape,
    Type,
    Hand,
    Zoom,
}

/// Foreground and background colors, straight sRGB with alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub foreground: [u8; 4],
    pub background: [u8; 4],
}

impl Default for Colors {
    fn default() -> Self {
        Colors { foreground: [0, 0, 0, 255], background: [255, 255, 255, 255] }
    }
}

impl Colors {
    pub fn fg32(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(self.foreground[0], self.foreground[1], self.foreground[2], self.foreground[3])
    }
    pub fn bg32(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(self.background[0], self.background[1], self.background[2], self.background[3])
    }
}

/// What a tool can reach while handling an event.
pub struct ToolCtx<'a> {
    pub project: &'a mut Project,
    pub colors: &'a mut Colors,
    pub cache: &'a RenderCache,
    /// Screen points per document pixel.
    pub zoom: f32,
    /// A message for the status bar.
    pub status: &'a mut Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Press,
    Drag,
    Release,
    Hover,
    DoubleClick,
}

#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    /// Document coordinates (fractional pixels).
    pub pos: (f64, f64),
    /// Where the current press started, in document coordinates.
    pub press_origin: (f64, f64),
    pub modifiers: Modifiers,
    /// Pen pressure 0…1 when known, otherwise 1.
    pub pressure: f32,
}

pub trait Tool {
    fn kind(&self) -> ToolKind;
    fn name(&self) -> &'static str;
    /// A short glyph for the toolbar button.
    fn icon(&self) -> &'static str;
    /// The single-key shortcut (lowercase), as in Photoshop.
    fn shortcut(&self) -> Option<Key> {
        None
    }
    /// The tool's settings, shown in the options bar above the canvas.
    fn options_ui(&mut self, _ui: &mut egui::Ui, _ctx: &mut ToolCtx) {}
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx);
    /// A key press while the canvas has focus; return true if the tool used it.
    fn key(&mut self, _key: Key, _modifiers: Modifiers, _ctx: &mut ToolCtx) -> bool {
        false
    }
    /// Draws handles, outlines and previews over the canvas.
    fn overlay(&self, _painter: &Painter, _view: &ViewTransform, _project: &Project, _hover: Option<(f64, f64)>) {}
    fn cursor(&self, _project: &Project, _hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        CursorIcon::Crosshair
    }
    /// Commits any in-progress interaction (switching tools, pressing Enter).
    fn commit(&mut self, _ctx: &mut ToolCtx) {}
    /// Abandons any in-progress interaction (Escape).
    fn cancel(&mut self, _ctx: &mut ToolCtx) {}
    /// Turns on transform handles (Free Transform); only the Move tool has them.
    fn set_transform_controls(&mut self, _on: bool) {}
    /// Whether the tool wants every key (while typing text, for example).
    fn captures_keyboard(&self) -> bool {
        false
    }
}

pub struct Tools {
    pub list: Vec<Box<dyn Tool>>,
    pub current: ToolKind,
    /// The tool to return to when a temporary one (Space for Hand) is released.
    pub previous: Option<ToolKind>,
}

impl Tools {
    pub fn new() -> Self {
        let list: Vec<Box<dyn Tool>> = vec![
            Box::new(move_tool::MoveTool::default()),
            Box::new(marquee::MarqueeTool::default()),
            Box::new(lasso::LassoTool::default()),
            Box::new(magic_wand::MagicWandTool::default()),
            Box::new(crop::CropTool::default()),
            Box::new(eyedropper::Eyedropper::default()),
            Box::new(spot_healing::SpotHealingTool::default()),
            Box::new(brush::BrushTool::default()),
            Box::new(eraser::EraserTool::default()),
            Box::new(clone_stamp::CloneStampTool::default()),
            Box::new(blur::BlurTool::default()),
            Box::new(gradient::GradientTool::default()),
            Box::new(paint_bucket::PaintBucketTool::default()),
            Box::new(type_tool::TypeTool::default()),
            Box::new(shape::ShapeTool::default()),
            Box::new(hand::HandTool),
            Box::new(hand::ZoomTool),
        ];
        Tools { list, current: ToolKind::Move, previous: None }
    }

    pub fn get(&self, kind: ToolKind) -> Option<&dyn Tool> {
        self.list.iter().find(|t| t.kind() == kind).map(|t| t.as_ref())
    }

    pub fn get_mut(&mut self, kind: ToolKind) -> Option<&mut Box<dyn Tool>> {
        self.list.iter_mut().find(|t| t.kind() == kind)
    }

    pub fn current_mut(&mut self) -> &mut Box<dyn Tool> {
        let kind = self.current;
        let index = self.list.iter().position(|t| t.kind() == kind).unwrap_or(0);
        &mut self.list[index]
    }

    pub fn current(&self) -> &dyn Tool {
        self.get(self.current).unwrap_or_else(|| self.list[0].as_ref())
    }
}

/// Document-space helpers shared by tools.
pub fn layer_pixel_at(project: &Project, id: crate::doc::Id, pos: (f64, f64)) -> Option<(u32, u32)> {
    let layer = project.doc.layer(id)?;
    let image = layer.image.as_ref()?;
    let inverse = layer.transform.pixel_to_document(image.width(), image.height()).inverse()?;
    let (u, v) = inverse.apply(pos.0, pos.1);
    (u >= 0.0 && v >= 0.0 && u < image.width() as f64 && v < image.height() as f64).then_some((u as u32, v as u32))
}
