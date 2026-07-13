//! Input sources: keyboard, on-screen touch gamepad, and (later) the BLE
//! GATT controller. All of them produce a [`ButtonStates`].

use gb_core::Button;

pub const ALL_BUTTONS: [Button; 8] = [
    Button::Right,
    Button::Left,
    Button::Up,
    Button::Down,
    Button::A,
    Button::B,
    Button::Select,
    Button::Start,
];

pub type ButtonStates = [bool; 8];

pub fn merge(a: ButtonStates, b: ButtonStates) -> ButtonStates {
    let mut out = [false; 8];
    for i in 0..8 {
        out[i] = a[i] || b[i];
    }
    out
}

/// External controller feed. The ESP32-H2 GATT client will implement this.
pub trait ControllerLink {
    /// Latest button snapshot, or None when no controller is connected.
    fn poll(&mut self) -> Option<ButtonStates>;
    fn status(&self) -> String;
}

pub struct NullController;

impl ControllerLink for NullController {
    fn poll(&mut self) -> Option<ButtonStates> {
        None
    }
    fn status(&self) -> String {
        "no controller".into()
    }
}

pub fn keyboard(ctx: &egui::Context) -> ButtonStates {
    use egui::Key;
    ctx.input(|i| {
        [
            i.key_down(Key::ArrowRight),
            i.key_down(Key::ArrowLeft),
            i.key_down(Key::ArrowUp),
            i.key_down(Key::ArrowDown),
            i.key_down(Key::X),
            i.key_down(Key::Z),
            i.key_down(Key::Backspace),
            i.key_down(Key::Enter),
        ]
    })
}

/// Tracks active touch points (and the mouse) for virtual-gamepad hit tests.
#[derive(Default)]
pub struct TouchTracker {
    touches: std::collections::HashMap<u64, egui::Pos2>,
}

impl TouchTracker {
    pub fn update(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            for ev in &i.raw.events {
                if let egui::Event::Touch { id, phase, pos, .. } = ev {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.touches.insert(id.0, *pos);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.touches.remove(&id.0);
                        }
                    }
                }
            }
        });
    }

    fn points(&self, ctx: &egui::Context) -> Vec<egui::Pos2> {
        let mut pts: Vec<egui::Pos2> = self.touches.values().copied().collect();
        ctx.input(|i| {
            if i.pointer.primary_down()
                && let Some(p) = i.pointer.interact_pos()
            {
                pts.push(p);
            }
        });
        pts
    }
}

/// Draw the virtual gamepad into `ui` and return pressed states.
pub fn virtual_gamepad(ui: &mut egui::Ui, tracker: &TouchTracker) -> ButtonStates {
    use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, pos2, vec2};

    let height = 190.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    let points = tracker.points(ui.ctx());
    let hit = |r: Rect| points.iter().any(|p| r.contains(*p));

    let mut states = [false; 8];
    let base = Color32::from_gray(60);
    let base_hit = Color32::from_gray(110);
    let text_col = Color32::from_gray(220);

    // D-pad on the left: three-rect cross.
    let pad_c = pos2(rect.left() + 95.0, rect.center().y);
    let arm = 34.0;
    let thick = 40.0;
    let dpad = [
        (Button::Up, Rect::from_center_size(pos2(pad_c.x, pad_c.y - arm), vec2(thick, arm * 1.6))),
        (Button::Down, Rect::from_center_size(pos2(pad_c.x, pad_c.y + arm), vec2(thick, arm * 1.6))),
        (Button::Left, Rect::from_center_size(pos2(pad_c.x - arm, pad_c.y), vec2(arm * 1.6, thick))),
        (Button::Right, Rect::from_center_size(pos2(pad_c.x + arm, pad_c.y), vec2(arm * 1.6, thick))),
    ];
    for (btn, r) in dpad {
        let pressed = hit(r);
        set(&mut states, btn, pressed);
        painter.rect_filled(r, CornerRadius::same(8), if pressed { base_hit } else { base });
    }

    // A / B on the right.
    let ab_c = pos2(rect.right() - 95.0, rect.center().y);
    let radius = 34.0;
    let ab = [
        (Button::B, pos2(ab_c.x - 44.0, ab_c.y + 18.0), "B"),
        (Button::A, pos2(ab_c.x + 44.0, ab_c.y - 18.0), "A"),
    ];
    for (btn, c, label) in ab {
        let r = Rect::from_center_size(c, vec2(radius * 2.0, radius * 2.0));
        let pressed = hit(r);
        set(&mut states, btn, pressed);
        painter.circle_filled(c, radius, if pressed { base_hit } else { base });
        painter.text(c, Align2::CENTER_CENTER, label, FontId::proportional(22.0), text_col);
    }

    // Start / Select pills in the middle.
    let pills = [
        (Button::Select, pos2(rect.center().x - 45.0, rect.bottom() - 30.0), "SELECT"),
        (Button::Start, pos2(rect.center().x + 45.0, rect.bottom() - 30.0), "START"),
    ];
    for (btn, c, label) in pills {
        let r = Rect::from_center_size(c, vec2(74.0, 26.0));
        let pressed = hit(r);
        set(&mut states, btn, pressed);
        painter.rect_filled(r, CornerRadius::same(13), if pressed { base_hit } else { base });
        painter.rect_stroke(
            r,
            CornerRadius::same(13),
            Stroke::new(1.0, Color32::from_gray(90)),
            egui::StrokeKind::Inside,
        );
        painter.text(c, Align2::CENTER_CENTER, label, FontId::proportional(11.0), text_col);
    }

    states
}

fn set(states: &mut ButtonStates, b: Button, v: bool) {
    let idx = ALL_BUTTONS.iter().position(|&x| x == b).unwrap();
    states[idx] = v;
}
