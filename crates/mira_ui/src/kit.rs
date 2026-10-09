//! A small set of controls for games, with a look that a game can change.
//!
//! Armature supplies how controls behave and how they are laid out; this supplies how they
//! look, from a [`Theme`]: a few colours and sizes. A game uses the default, changes some of
//! it, or writes its own controls the same way these are written.
//!
//! ```ignore
//! fn view(&self) -> Element<Msg> {
//!     anchored(Anchor::TopRight, panel(
//!         column().spacing(10.0)
//!             .push(heading("Round"))
//!             .push(bar(self.clock / self.limit))
//!             .push(slider(10.0..=120.0, self.limit, Msg::Limit))
//!             .push(toggle("Show rules", self.rules, Msg::Rules))
//!             .push(field("Your name", &self.name, Msg::Name).on_submit(Msg::Join))
//!             .push(button("Restart", Msg::Restart)),
//!     ))
//! }
//!
//! fn style(&self, _: Scheme) -> Style {
//!     Theme::default().style()
//! }
//! ```

use std::ops::RangeInclusive;

use armature::{
    controls::{byte_at, char_at, FieldAction, FieldLogic, FieldState, SliderLogic, SliderState},
    Color, CursorIcon, Cx, DrawCx, Element, Event, EventCx, FontFamily, Key, Limits, Point,
    PointerButton, Rect, Size, Status, Style, TextLayout, TextStyle, Widget,
};

pub use armature::widgets::{column, label, row, stack};
pub use armature::{Align, Padding};

/// How the kit's controls look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Behind a panel's contents. Usually see-through, so the game shows behind it.
    pub panel: Color,
    /// A panel's hairline edge.
    pub edge: Color,
    pub text: Color,
    /// Text that matters less: captions, values.
    pub dim: Color,
    /// What can be pressed, at rest.
    pub control: Color,
    /// The colour of what is on, filled or chosen.
    pub accent: Color,
    /// Corner radius of panels; controls use about half.
    pub radius: f32,
    /// Size of ordinary text, in logical pixels.
    pub size: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            panel: Color::hex(0x101216).with_alpha(0.86),
            edge: Color::WHITE.with_alpha(0.14),
            text: Color::WHITE.with_alpha(0.95),
            dim: Color::WHITE.with_alpha(0.55),
            control: Color::hex(0x2a2f3a),
            accent: Color::hex(0x7ee08a),
            radius: 12.0,
            size: 13.0,
        }
    }
}

impl Theme {
    /// The style to return from `App::style`, so the kit's controls find this theme.
    pub fn style(self) -> Style {
        Style::new(self).content(self.text).text(self.body())
    }

    /// The theme the interface was given, or the default.
    fn of(cx: &Cx) -> Self {
        cx.style::<Theme>().copied().unwrap_or_default()
    }

    fn body(&self) -> TextStyle {
        TextStyle {
            size: self.size,
            weight: 500,
            family: FontFamily::Sans,
            line_height: 1.35,
            letter_spacing: 0.0,
        }
    }

    fn strong(&self) -> TextStyle {
        TextStyle {
            weight: 700,
            ..self.body()
        }
    }
}

/// Lightens a colour a little: for what the pointer is over.
fn lift(colour: Color, by: f32) -> Color {
    let [r, g, b, a] = colour.to_rgba8().map(|c| c as f32 / 255.0);
    Color::rgba(
        r + (1.0 - r) * by,
        g + (1.0 - g) * by,
        b + (1.0 - b) * by,
        a,
    )
}

// --- where an interface sits on the screen ---

/// A corner, an edge or the middle of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Centre,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

struct Anchored<M> {
    anchor: Anchor,
    margin: f32,
    child: [Element<M>; 1],
}

/// Places `child` at a corner, an edge or the middle of the screen, a margin in from it.
/// The rest of the screen stays the game's: only what `child` draws is interface.
pub fn anchored<M: 'static>(anchor: Anchor, child: impl Into<Element<M>>) -> Element<M> {
    Element::new(Anchored {
        anchor,
        margin: 14.0,
        child: [child.into()],
    })
}

impl<M: 'static> Widget<M> for Anchored<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let room = Size::new(
            (limits.max.w - self.margin * 2.0).max(0.0),
            (limits.max.h - self.margin * 2.0).max(0.0),
        );
        let size = self.child[0].layout(cx, Limits::loose(room));
        use Anchor::*;
        let x = match self.anchor {
            TopLeft | Left | BottomLeft => self.margin,
            Top | Centre | Bottom => (limits.max.w - size.w) * 0.5,
            TopRight | Right | BottomRight => limits.max.w - size.w - self.margin,
        };
        let y = match self.anchor {
            TopLeft | Top | TopRight => self.margin,
            Left | Centre | Right => (limits.max.h - size.h) * 0.5,
            BottomLeft | Bottom | BottomRight => limits.max.h - size.h - self.margin,
        };
        self.child[0].set_position(Point::new(x, y));
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        self.child[0].draw(cx);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        self.child[0].event(cx, event)
    }
}

// --- panel ---

struct Panel<M> {
    padding: f32,
    child: [Element<M>; 1],
}

/// A rounded, see-through backing for a group of controls.
pub fn panel<M: 'static>(child: impl Into<Element<M>>) -> Element<M> {
    Element::new(Panel {
        padding: 14.0,
        child: [child.into()],
    })
}

impl<M: 'static> Widget<M> for Panel<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let pad = self.padding * 2.0;
        let room = Size::new((limits.max.w - pad).max(0.0), (limits.max.h - pad).max(0.0));
        let size = self.child[0].layout(cx, Limits::loose(room));
        self.child[0].set_position(Point::new(self.padding, self.padding));
        Size::new(size.w + pad, size.h + pad)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        cx.scene
            .fill(bounds, theme.radius, theme.panel, Some((1.0, theme.edge)));
        self.child[0].draw(cx);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        self.child[0].event(cx, event)
    }
}

// --- text ---

struct Text {
    content: String,
    strong: bool,
    dim: bool,
    layout: Option<TextLayout>,
}

/// A line that names what is below it.
pub fn heading<M: 'static>(content: impl Into<String>) -> Element<M> {
    Element::new(Text {
        content: content.into(),
        strong: true,
        dim: false,
        layout: None,
    })
}

/// Quieter text: a value, a hint.
pub fn caption<M: 'static>(content: impl Into<String>) -> Element<M> {
    Element::new(Text {
        content: content.into(),
        strong: false,
        dim: true,
        layout: None,
    })
}

impl<M> Widget<M> for Text {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let theme = Theme::of(cx);
        let style = if self.strong {
            theme.strong()
        } else {
            theme.body()
        };
        let layout = cx.text().layout(&self.content, &style, Some(limits.max.w));
        let size = layout.size();
        self.layout = Some(layout);
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        if let Some(layout) = &self.layout {
            let colour = if self.dim { theme.dim } else { theme.text };
            cx.scene
                .text(layout, Point::new(bounds.x, bounds.y), colour);
        }
    }
}

// --- button ---

/// What the pointer is doing to a control.
#[derive(Default)]
struct Touch {
    over: bool,
    held: bool,
}

struct Button<M> {
    label: String,
    on_press: M,
    text: Option<TextLayout>,
}

/// A button that sends `on_press` when clicked, or with Enter or Space when it has focus.
pub fn button<M: Clone + 'static>(label: impl Into<String>, on_press: M) -> Element<M> {
    Element::new(Button {
        label: label.into(),
        on_press,
        text: None,
    })
}

impl<M: Clone + 'static> Widget<M> for Button<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let theme = Theme::of(cx);
        let text = cx.text().layout(&self.label, &theme.strong(), None);
        let size = Size::new(text.size().w + 28.0, text.size().h + 14.0);
        self.text = Some(text);
        Size::new(size.w.min(limits.max.w), size.h)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let focus = cx.focus_visible();
        let touch = cx.state::<Touch>();
        let fill = match (touch.held, touch.over) {
            (true, _) => theme.accent.with_alpha(0.55),
            (false, true) => lift(theme.control, 0.14),
            (false, false) => theme.control,
        };
        let edge = if focus {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        cx.scene.fill(bounds, theme.radius * 0.5, fill, Some(edge));
        if let Some(text) = &self.text {
            let at = Point::new(
                bounds.x + (bounds.w - text.size().w) * 0.5,
                bounds.y + (bounds.h - text.size().h) * 0.5,
            );
            cx.scene.text(text, at, theme.text);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        match event {
            Event::PointerMoved { pos } => {
                let over = bounds.contains(*pos);
                if over {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                if std::mem::replace(&mut cx.state::<Touch>().over, over) != over {
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } if bounds.contains(*pos) => {
                cx.state::<Touch>().held = true;
                cx.request_redraw();
                Status::Captured
            }
            Event::PointerReleased {
                pos,
                button: PointerButton::Primary,
            } => {
                if std::mem::take(&mut cx.state::<Touch>().held) {
                    cx.request_redraw();
                    if bounds.contains(*pos) {
                        cx.emit(self.on_press.clone());
                    }
                    return Status::Captured;
                }
                Status::Ignored
            }
            Event::Key(key)
                if key.pressed && cx.is_focused() && matches!(key.key, Key::Enter | Key::Space) =>
            {
                cx.emit(self.on_press.clone());
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

// --- toggle ---

struct Toggle<M> {
    label: String,
    on: bool,
    on_change: fn(bool) -> M,
    text: Option<TextLayout>,
}

const SWITCH: Size = Size { w: 34.0, h: 20.0 };

/// A labelled switch. Clicking it, or Enter or Space with focus, sends the other state.
pub fn toggle<M: 'static>(
    label: impl Into<String>,
    on: bool,
    on_change: fn(bool) -> M,
) -> Element<M> {
    Element::new(Toggle {
        label: label.into(),
        on,
        on_change,
        text: None,
    })
}

impl<M: 'static> Widget<M> for Toggle<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let theme = Theme::of(cx);
        let text = cx.text().layout(&self.label, &theme.body(), None);
        let size = Size::new(SWITCH.w + 10.0 + text.size().w, SWITCH.h.max(text.size().h));
        self.text = Some(text);
        Size::new(size.w.min(limits.max.w), size.h)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let focus = cx.focus_visible();
        let track = Rect::new(
            bounds.x,
            bounds.y + (bounds.h - SWITCH.h) * 0.5,
            SWITCH.w,
            SWITCH.h,
        );
        let fill = if self.on { theme.accent } else { theme.control };
        let edge = if focus {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        cx.scene.fill(track, SWITCH.h * 0.5, fill, Some(edge));
        let knob = SWITCH.h - 6.0;
        let x = if self.on {
            track.x + track.w - knob - 3.0
        } else {
            track.x + 3.0
        };
        cx.scene.fill(
            Rect::new(x, track.y + 3.0, knob, knob),
            knob * 0.5,
            Color::WHITE,
            None,
        );
        if let Some(text) = &self.text {
            let at = Point::new(
                track.x + track.w + 10.0,
                bounds.y + (bounds.h - text.size().h) * 0.5,
            );
            cx.scene.text(text, at, theme.text);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        match event {
            Event::PointerMoved { pos } if bounds.contains(*pos) => {
                cx.set_cursor(CursorIcon::Pointer);
                Status::Ignored
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } if bounds.contains(*pos) => {
                cx.emit((self.on_change)(!self.on));
                Status::Captured
            }
            Event::Key(key)
                if key.pressed && cx.is_focused() && matches!(key.key, Key::Enter | Key::Space) =>
            {
                cx.emit((self.on_change)(!self.on));
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

// --- slider ---

struct Slider<M> {
    logic: SliderLogic,
    on_change: fn(f32) -> M,
    width: f32,
}

const KNOB: f32 = 16.0;

/// A slider over a range. Dragging, clicking the track and the arrow keys (with focus) all
/// send the new value.
pub fn slider<M: 'static>(
    range: RangeInclusive<f32>,
    value: f32,
    on_change: fn(f32) -> M,
) -> Element<M> {
    Element::new(Slider {
        logic: SliderLogic::new(range, value),
        on_change,
        width: 180.0,
    })
}

impl<M: 'static> Widget<M> for Slider<M> {
    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        Size::new(self.width.min(limits.max.w), 22.0)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let focus = cx.focus_visible();
        let held = cx.state::<SliderState>().dragging;
        let middle = bounds.y + bounds.h * 0.5;
        let x = bounds.x + (bounds.w - KNOB) * self.logic.fraction();
        cx.scene.fill(
            Rect::new(bounds.x, middle - 2.5, bounds.w, 5.0),
            2.5,
            theme.control,
            None,
        );
        cx.scene.fill(
            Rect::new(bounds.x, middle - 2.5, x - bounds.x + KNOB * 0.5, 5.0),
            2.5,
            theme.accent,
            None,
        );
        let edge = if focus || held {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        cx.scene.fill(
            Rect::new(x, middle - KNOB * 0.5, KNOB, KNOB),
            KNOB * 0.5,
            Color::WHITE,
            Some(edge),
        );
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        let (status, change) =
            self.logic
                .event(cx, event, bounds, bounds.x + KNOB * 0.5, bounds.w - KNOB);
        if let Some(value) = change.value {
            cx.emit((self.on_change)(value));
        }
        status
    }
}

// --- field ---

/// A line of text to type into. Made by [`field`].
pub struct Field<M> {
    value: String,
    hint: String,
    on_input: fn(String) -> M,
    on_submit: Option<M>,
    width: f32,
    autofocus: bool,
    text: Option<TextLayout>,
    hint_text: Option<TextLayout>,
}

const FIELD_INSET: f32 = 8.0;

/// A line of text to type into. The interface keeps the text: every edit sends the whole
/// new text through `on_input`, and what the interface then passes as `value` is what shows.
/// `hint` shows, dimmed, while there is none. Clicking gives it the keyboard, and clicking
/// elsewhere or Escape takes it away; while it has it, what is typed is the field's and not
/// the game's.
pub fn field<M: Clone + 'static>(
    hint: impl Into<String>,
    value: impl Into<String>,
    on_input: fn(String) -> M,
) -> Field<M> {
    Field {
        value: value.into(),
        hint: hint.into(),
        on_input,
        on_submit: None,
        width: 180.0,
        autofocus: false,
        text: None,
        hint_text: None,
    }
}

impl<M> Field<M> {
    /// What to send when Enter is pressed.
    pub fn on_submit(mut self, message: M) -> Self {
        self.on_submit = Some(message);
        self
    }

    /// How wide the field is, in logical pixels. 180 unless said.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Takes the keyboard, with its text selected, the first time it appears: for a field
    /// that is shown because someone asked to type.
    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self
    }

    fn logic(&self) -> FieldLogic<'_> {
        FieldLogic {
            value: &self.value,
            secure: false,
            arrows: false,
        }
    }

    /// Where the text starts, given how far it has slid.
    fn origin(&self, bounds: Rect, scroll: f32) -> Point {
        let height = self.text.as_ref().map_or(0.0, |text| text.size().h);
        Point::new(
            bounds.x + FIELD_INSET - scroll,
            bounds.y + ((bounds.h - height) * 0.5).round(),
        )
    }

    /// How far along the text the caret before this character is.
    fn caret_x(&self, character: usize) -> f32 {
        self.text
            .as_ref()
            .map_or(0.0, |text| text.caret(byte_at(&self.value, character)).x)
    }

    /// The character nearest a point.
    fn hit(&self, bounds: Rect, scroll: f32, point: Point) -> usize {
        let Some(text) = &self.text else { return 0 };
        let origin = self.origin(bounds, scroll);
        let across = Point::new(point.x - origin.x, text.line_height() * 0.5);
        char_at(&self.value, text.hit(across))
    }
}

impl<M: Clone + 'static> From<Field<M>> for Element<M> {
    fn from(field: Field<M>) -> Self {
        Element::new(field)
    }
}

impl<M: Clone + 'static> Widget<M> for Field<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = Theme::of(cx).body();
        self.text = Some(cx.text().layout(&self.value, &style, None));
        self.hint_text = Some(cx.text().layout(&self.hint, &style, None));
        self.logic().sync(cx, self.autofocus);
        let height = (style.size * style.line_height + 10.0).round();
        Size::new(self.width.min(limits.max.w), height)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let now = cx.now();
        let focused = cx.is_focused();
        let (cursor, anchor, hovered, blink) = {
            let state = cx.state::<FieldState>();
            (state.cursor, state.anchor, state.hovered, state.blink(now))
        };
        let edge = if focused {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        let fill = if hovered && !focused {
            lift(theme.control, 0.06)
        } else {
            theme.control
        };
        cx.scene.fill(bounds, theme.radius * 0.5, fill, Some(edge));

        let inner = bounds.w - FIELD_INSET * 2.0;
        let caret = self.caret_x(cursor);
        let scroll = cx.state::<FieldState>().keep_caret_visible(caret, inner);
        let origin = self.origin(bounds, scroll);
        let height = self.text.as_ref().map_or(0.0, |text| text.size().h);
        cx.scene.push_clip(Rect::new(
            bounds.x + FIELD_INSET - 1.0,
            bounds.y,
            inner + 3.0,
            bounds.h,
        ));
        if focused && cursor != anchor {
            let (from, to) = (
                self.caret_x(cursor.min(anchor)),
                self.caret_x(cursor.max(anchor)),
            );
            cx.scene.fill(
                Rect::new(origin.x + from, origin.y, to - from, height),
                3.0,
                theme.accent.with_alpha(0.3),
                None,
            );
        }
        match (&self.text, &self.hint_text) {
            (_, Some(hint)) if self.value.is_empty() => cx.scene.text(hint, origin, theme.dim),
            (Some(text), _) => cx.scene.text(text, origin, theme.text),
            _ => {}
        }
        if focused {
            let (showing, next) = blink;
            if showing {
                cx.scene.fill(
                    Rect::new(
                        (origin.x + caret).round() - 0.75,
                        origin.y + 1.0,
                        1.5,
                        height - 2.0,
                    ),
                    0.75,
                    theme.accent,
                    None,
                );
            }
            cx.request_redraw_after(next);
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        let scroll = cx.state::<FieldState>().scroll;
        let (status, action) = self
            .logic()
            .event(cx, event, bounds, |point| self.hit(bounds, scroll, point));
        match action {
            Some(FieldAction::Edit(value)) => {
                let message = (self.on_input)(value.clone());
                self.value = value;
                cx.emit(message);
            }
            Some(FieldAction::Submit) => {
                if let Some(message) = self.on_submit.clone() {
                    cx.emit(message);
                }
            }
            Some(FieldAction::Cancel | FieldAction::Arrow(_)) | None => {}
        }
        status
    }
}

// --- bar ---

struct Bar {
    fraction: f32,
    width: f32,
}

/// A bar filled to a fraction of its length: health, progress, a clock.
pub fn bar<M: 'static>(fraction: f32) -> Element<M> {
    Element::new(Bar {
        fraction: if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        },
        width: 180.0,
    })
}

impl<M> Widget<M> for Bar {
    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        Size::new(self.width.min(limits.max.w), 10.0)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        cx.scene.fill(
            bounds,
            bounds.h * 0.5,
            theme.control,
            Some((1.0, theme.edge)),
        );
        if self.fraction > 0.0 {
            let filled = Rect::new(
                bounds.x,
                bounds.y,
                (bounds.w * self.fraction).max(bounds.h),
                bounds.h,
            );
            cx.scene.fill(filled, bounds.h * 0.5, theme.accent, None);
        }
    }
}
