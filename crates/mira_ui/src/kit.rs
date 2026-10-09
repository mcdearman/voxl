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
//!             .push(choice(["Easy", "Fair", "Hard"], self.level, Msg::Level))
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
    controls::{
        byte_at, char_at, FieldAction, FieldLogic, FieldState, ScrollLogic, ScrollState,
        ScrollStep, SliderLogic, SliderState,
    },
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
    on_cancel: Option<M>,
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
        on_cancel: None,
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

    /// What to send when Escape is pressed, which also gives up the keyboard.
    pub fn on_cancel(mut self, message: M) -> Self {
        self.on_cancel = Some(message);
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
            Some(FieldAction::Cancel) => {
                if let Some(message) = self.on_cancel.clone() {
                    cx.emit(message);
                }
            }
            Some(FieldAction::Arrow(_)) | None => {}
        }
        status
    }
}

// --- tabs ---

/// Which of a control's parts the pointer is over.
#[derive(Default)]
struct Over(Option<usize>);

/// One step along a row or a list for an arrow key, staying inside it.
fn stepped(chosen: usize, by: i32, count: usize) -> Option<usize> {
    let to = chosen.checked_add_signed(by as isize)?;
    (to < count).then_some(to)
}

struct Tabs<M> {
    labels: Vec<String>,
    chosen: usize,
    on_choose: fn(usize) -> M,
    texts: Vec<TextLayout>,
}

const TAB_INSET: f32 = 12.0;

/// A row of tabs, one of them chosen: the pages of a menu, or a choice between a few things
/// that are all worth seeing at once. A click sends the tab's place in the row, and with
/// focus so do the left and right arrows.
pub fn tabs<M: 'static, L: Into<String>>(
    labels: impl IntoIterator<Item = L>,
    chosen: usize,
    on_choose: fn(usize) -> M,
) -> Element<M> {
    Element::new(Tabs {
        labels: labels.into_iter().map(Into::into).collect(),
        chosen,
        on_choose,
        texts: Vec::new(),
    })
}

impl<M> Tabs<M> {
    /// Where each tab is, in a row from `bounds`' left edge.
    fn places(&self, bounds: Rect) -> Vec<Rect> {
        let mut x = bounds.x;
        self.texts
            .iter()
            .map(|text| {
                let place = Rect::new(x, bounds.y, text.size().w + TAB_INSET * 2.0, bounds.h);
                x += place.w;
                place
            })
            .collect()
    }
}

impl<M: 'static> Widget<M> for Tabs<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let theme = Theme::of(cx);
        let style = theme.strong();
        self.texts = self
            .labels
            .iter()
            .map(|label| cx.text().layout(label, &style, None))
            .collect();
        let width: f32 = self
            .texts
            .iter()
            .map(|text| text.size().w + TAB_INSET * 2.0)
            .sum();
        let height = (style.size * style.line_height + 12.0).round();
        Size::new(width.min(limits.max.w), height)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let over = cx.state::<Over>().0;
        let edge = if cx.focus_visible() {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        let radius = theme.radius * 0.5;
        cx.scene.fill(bounds, radius, theme.control, Some(edge));
        for (index, (place, text)) in self.places(bounds).into_iter().zip(&self.texts).enumerate() {
            let chosen = index == self.chosen;
            if chosen || over == Some(index) {
                let inner = Rect::new(place.x + 2.0, place.y + 2.0, place.w - 4.0, place.h - 4.0);
                let fill = lift(theme.control, if chosen { 0.16 } else { 0.07 });
                cx.scene.fill(inner, (radius - 2.0).max(0.0), fill, None);
            }
            if chosen {
                cx.scene.fill(
                    Rect::new(
                        place.x + TAB_INSET,
                        place.y + place.h - 4.0,
                        text.size().w,
                        2.0,
                    ),
                    1.0,
                    theme.accent,
                    None,
                );
            }
            let at = Point::new(
                place.x + TAB_INSET,
                place.y + ((place.h - text.size().h) * 0.5).round(),
            );
            cx.scene
                .text(text, at, if chosen { theme.text } else { theme.dim });
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        let at = |pos: Point| {
            self.places(bounds)
                .iter()
                .position(|place| place.contains(pos))
        };
        match event {
            Event::PointerMoved { pos } => {
                let over = at(*pos);
                if over.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                if std::mem::replace(&mut cx.state::<Over>().0, over) != over {
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } => match at(*pos) {
                Some(index) => {
                    cx.emit((self.on_choose)(index));
                    Status::Captured
                }
                None => Status::Ignored,
            },
            Event::Key(key) if key.pressed && cx.is_focused() => {
                let by = match key.key {
                    Key::Left => -1,
                    Key::Right => 1,
                    _ => return Status::Ignored,
                };
                if let Some(to) = stepped(self.chosen, by, self.labels.len()) {
                    cx.emit((self.on_choose)(to));
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

// --- choice ---

struct Choice<M> {
    options: Vec<String>,
    chosen: usize,
    on_choose: fn(usize) -> M,
    width: f32,
    text: Option<TextLayout>,
}

/// The width of the arrow at each end of a choice.
const ARROW: f32 = 28.0;

/// One of several options, shown one at a time between two arrows: a setting such as a
/// difficulty or a screen size. The arrows, and with focus the left and right arrow keys,
/// send the option before or after; it stops at each end.
pub fn choice<M: 'static, L: Into<String>>(
    options: impl IntoIterator<Item = L>,
    chosen: usize,
    on_choose: fn(usize) -> M,
) -> Element<M> {
    Element::new(Choice {
        options: options.into_iter().map(Into::into).collect(),
        chosen,
        on_choose,
        width: 180.0,
        text: None,
    })
}

impl<M> Choice<M> {
    /// The arrow under a point: back or on.
    fn arrow(&self, bounds: Rect, pos: Point) -> Option<i32> {
        if !bounds.contains(pos) {
            None
        } else if pos.x < bounds.x + ARROW {
            Some(-1)
        } else if pos.x > bounds.x + bounds.w - ARROW {
            Some(1)
        } else {
            None
        }
    }
}

impl<M: 'static> Widget<M> for Choice<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = Theme::of(cx).body();
        let width = self.width.min(limits.max.w);
        self.text = self
            .options
            .get(self.chosen)
            .map(|option| cx.text().layout(option, &style, Some(width - ARROW * 2.0)));
        Size::new(width, (style.size * style.line_height + 10.0).round())
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let over = cx.state::<Over>().0;
        let edge = if cx.focus_visible() {
            (2.0, theme.accent)
        } else {
            (1.0, theme.edge)
        };
        cx.scene
            .fill(bounds, theme.radius * 0.5, theme.control, Some(edge));
        let middle = bounds.y + bounds.h * 0.5;
        for (index, by) in [-1, 1].into_iter().enumerate() {
            // An arrow that leads nowhere is faint.
            let leads = stepped(self.chosen, by, self.options.len()).is_some();
            let colour = match (leads, over == Some(index)) {
                (false, _) => theme.dim.with_alpha(0.25),
                (true, true) => theme.accent,
                (true, false) => theme.text,
            };
            let tip = if by < 0 {
                bounds.x + ARROW * 0.5 - 3.0
            } else {
                bounds.x + bounds.w - ARROW * 0.5 + 3.0
            };
            let back = tip - by as f32 * 6.0;
            cx.scene.polyline(
                &[
                    Point::new(back, middle - 5.0),
                    Point::new(tip, middle),
                    Point::new(back, middle + 5.0),
                ],
                1.8,
                colour,
            );
        }
        if let Some(text) = &self.text {
            let at = Point::new(
                bounds.x + ((bounds.w - text.size().w) * 0.5).round(),
                bounds.y + ((bounds.h - text.size().h) * 0.5).round(),
            );
            cx.scene.text(text, at, theme.text);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        let by = match event {
            Event::PointerMoved { pos } => {
                let over = self.arrow(bounds, *pos).map(|by| (by > 0) as usize);
                if over.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                if std::mem::replace(&mut cx.state::<Over>().0, over) != over {
                    cx.request_redraw();
                }
                return Status::Ignored;
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } if bounds.contains(*pos) => self.arrow(bounds, *pos),
            Event::Key(key) if key.pressed && cx.is_focused() => match key.key {
                Key::Left => Some(-1),
                Key::Right => Some(1),
                _ => return Status::Ignored,
            },
            _ => return Status::Ignored,
        };
        if let Some(to) = by.and_then(|by| stepped(self.chosen, by, self.options.len())) {
            cx.emit((self.on_choose)(to));
        }
        Status::Captured
    }
}

// --- list ---

struct List<M> {
    items: Vec<String>,
    chosen: Option<usize>,
    on_choose: fn(usize) -> M,
    texts: Vec<TextLayout>,
    row: f32,
}

/// Rows of text to choose one of: saved games, an inventory, a list of servers. A click
/// sends the row's place in the list, and with focus so do the up and down arrows. A long
/// list goes inside [`scroll`].
pub fn list<M: 'static, L: Into<String>>(
    items: impl IntoIterator<Item = L>,
    chosen: Option<usize>,
    on_choose: fn(usize) -> M,
) -> Element<M> {
    Element::new(List {
        items: items.into_iter().map(Into::into).collect(),
        chosen,
        on_choose,
        texts: Vec::new(),
        row: 0.0,
    })
}

impl<M> List<M> {
    fn row_at(&self, bounds: Rect, pos: Point) -> Option<usize> {
        if !bounds.contains(pos) || self.row <= 0.0 {
            return None;
        }
        let row = ((pos.y - bounds.y) / self.row) as usize;
        (row < self.items.len()).then_some(row)
    }
}

impl<M: 'static> Widget<M> for List<M> {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = Theme::of(cx).body();
        let widest = if limits.max.w.is_finite() {
            limits.max.w
        } else {
            220.0
        };
        self.texts = self
            .items
            .iter()
            .map(|item| cx.text().layout(item, &style, Some(widest - 20.0)))
            .collect();
        self.row = (style.size * style.line_height + 8.0).round();
        let width = self
            .texts
            .iter()
            .map(|text| text.size().w + 20.0)
            .fold(180.0_f32.min(widest), f32::max);
        Size::new(width.min(widest), self.row * self.items.len() as f32)
    }

    fn focusable(&self) -> bool {
        true
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        let over = cx.state::<Over>().0;
        let focus = cx.focus_visible();
        for (index, text) in self.texts.iter().enumerate() {
            let row = Rect::new(
                bounds.x,
                bounds.y + self.row * index as f32,
                bounds.w,
                self.row,
            );
            let chosen = self.chosen == Some(index);
            if chosen {
                let edge = focus.then_some((1.0, theme.accent));
                cx.scene
                    .fill(row, theme.radius * 0.4, theme.accent.with_alpha(0.22), edge);
                cx.scene.fill(
                    Rect::new(row.x + 2.0, row.y + 5.0, 3.0, row.h - 10.0),
                    1.5,
                    theme.accent,
                    None,
                );
            } else if over == Some(index) {
                cx.scene
                    .fill(row, theme.radius * 0.4, lift(theme.control, 0.05), None);
            }
            let at = Point::new(
                row.x + 12.0,
                row.y + ((row.h - text.size().h) * 0.5).round(),
            );
            let colour = if chosen || over == Some(index) {
                theme.text
            } else {
                theme.dim
            };
            cx.scene.text(text, at, colour);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        match event {
            Event::PointerMoved { pos } => {
                let over = self.row_at(bounds, *pos);
                if over.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                if std::mem::replace(&mut cx.state::<Over>().0, over) != over {
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerPressed {
                pos,
                button: PointerButton::Primary,
            } => match self.row_at(bounds, *pos) {
                Some(row) => {
                    cx.request_focus();
                    cx.emit((self.on_choose)(row));
                    Status::Captured
                }
                None => Status::Ignored,
            },
            Event::Key(key) if key.pressed && cx.is_focused() => {
                let by = match key.key {
                    Key::Up => -1,
                    Key::Down => 1,
                    _ => return Status::Ignored,
                };
                let to = match self.chosen {
                    Some(chosen) => stepped(chosen, by, self.items.len()),
                    None => (!self.items.is_empty()).then_some(0),
                };
                if let Some(to) = to {
                    cx.emit((self.on_choose)(to));
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

// --- scroll ---

struct Scroll<M> {
    height: f32,
    logic: ScrollLogic,
    child: [Element<M>; 1],
}

/// Shows `child` through a window `height` tall, scrolled with the wheel or by dragging the
/// bar that appears at its right when there is more than fits.
pub fn scroll<M: 'static>(height: f32, child: impl Into<Element<M>>) -> Element<M> {
    Element::new(Scroll {
        height,
        logic: ScrollLogic::default(),
        child: [child.into()],
    })
}

impl<M> Scroll<M> {
    fn thumb(&self, bounds: Rect, offset: f32) -> Option<Rect> {
        self.logic.thumb(bounds, offset, 5.0, 2.0, 20.0)
    }
}

impl<M: 'static> Widget<M> for Scroll<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let inside = Limits::new(Size::new(0.0, 0.0), Size::new(limits.max.w, f32::INFINITY));
        let content = self.child[0].layout(cx, inside);
        self.logic.content = content.h;
        let size = Size::new(content.w, content.h.min(self.height).min(limits.max.h));
        let offset = self.logic.clamp(cx, size.h);
        self.child[0].set_position(Point::new(0.0, -offset));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = Theme::of(cx);
        let bounds = cx.bounds();
        cx.scene.push_clip(bounds);
        self.child[0].draw(cx);
        cx.scene.pop_clip();
        let (offset, held) = {
            let state = cx.state::<ScrollState>();
            (state.offset, state.hovered || state.drag.is_some())
        };
        if let Some(thumb) = self.thumb(bounds, offset) {
            let colour = theme.text.with_alpha(if held { 0.6 } else { 0.3 });
            cx.scene.fill(thumb, 2.5, colour, None);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        if let Event::Wheel { pos, delta } = event {
            if !bounds.contains(*pos) {
                return Status::Ignored;
            }
            // What is inside gets the wheel first, so a scroller inside a scroller works.
            let status = self.child[0].event(cx, event);
            if status == Status::Captured || !self.logic.wheel(cx, delta.y, bounds.h) {
                return status;
            }
            return Status::Captured;
        }
        let offset = cx.state::<ScrollState>().offset;
        match self
            .logic
            .pointer(cx, event, bounds, self.thumb(bounds, offset))
        {
            ScrollStep::Done(status) => status,
            ScrollStep::Pass => self.child[0].event(cx, event),
            // Rows scrolled out of sight are not under the pointer, wherever it is.
            ScrollStep::PassAway => self.child[0].event(
                cx,
                &Event::PointerMoved {
                    pos: Point::new(f32::MIN, f32::MIN),
                },
            ),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_keys_step_along_and_stop_at_the_ends() {
        assert_eq!(stepped(0, 1, 3), Some(1));
        assert_eq!(stepped(2, -1, 3), Some(1));
        assert_eq!(stepped(0, -1, 3), None);
        assert_eq!(stepped(2, 1, 3), None);
        assert_eq!(stepped(0, 1, 0), None);
    }

    #[test]
    fn a_choice_has_an_arrow_at_each_end() {
        let choice = Choice {
            options: vec!["a".into(), "b".into()],
            chosen: 0,
            on_choose: |index| index,
            width: 180.0,
            text: None,
        };
        let bounds = Rect::new(100.0, 50.0, 180.0, 28.0);
        assert_eq!(choice.arrow(bounds, Point::new(110.0, 60.0)), Some(-1));
        assert_eq!(choice.arrow(bounds, Point::new(270.0, 60.0)), Some(1));
        assert_eq!(choice.arrow(bounds, Point::new(190.0, 60.0)), None);
        assert_eq!(choice.arrow(bounds, Point::new(110.0, 90.0)), None);
    }

    #[test]
    fn a_list_knows_which_row_is_under_the_pointer() {
        let list = List {
            items: vec!["a".into(), "b".into(), "c".into()],
            chosen: None,
            on_choose: |index| index,
            texts: Vec::new(),
            row: 26.0,
        };
        let bounds = Rect::new(10.0, 100.0, 180.0, 78.0);
        assert_eq!(list.row_at(bounds, Point::new(20.0, 101.0)), Some(0));
        assert_eq!(list.row_at(bounds, Point::new(20.0, 153.0)), Some(2));
        assert_eq!(list.row_at(bounds, Point::new(20.0, 99.0)), None);
        assert_eq!(list.row_at(bounds, Point::new(5.0, 120.0)), None);
        // Scrolled up by a row and a half, the same point is further down the list.
        let scrolled = Rect::new(10.0, 100.0 - 39.0, 180.0, 78.0);
        assert_eq!(list.row_at(scrolled, Point::new(20.0, 101.0)), Some(1));
    }
}
