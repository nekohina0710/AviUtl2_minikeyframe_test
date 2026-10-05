use aviutl2_eframe::{eframe, egui};
use egui::{pos2, vec2, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke};
use mini_keyframe_core::{Ease, Store};
use std::time::{Duration, Instant};

const EM: f32 = 30.0; // 正方形の周りの余白

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub(crate) struct EasingApp {
    store: Store,
    drag_h: Option<usize>,
    pending: bool,
    last_save: Instant,
    msg: String,
}

impl EasingApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        Self {
            store: Store::open(),
            drag_h: None,
            pending: false,
            last_save: Instant::now(),
            msg: String::new(),
        }
    }

    fn flush(&mut self) {
        let lua = crate::LUA_PATH.get().map(|p| p.as_path());
        match self.store.save(lua) {
            Ok(()) => self.msg.clear(),
            Err(e) => self.msg = format!("保存に失敗: {e}"),
        }
        self.pending = false;
        self.last_save = Instant::now();
    }

    /// 編集UI。変更があったらtrueを返す。
    fn editor(&mut self, ui: &mut egui::Ui) -> bool {
        ui.heading("イージング");
        if !self.msg.is_empty() {
            ui.label(egui::RichText::new(&self.msg).color(egui::Color32::from_rgb(230, 120, 60)));
        }
        let sel = self.store.state.sel.clone();
        let (Some(id), Some(j)) = (sel.id, sel.key) else {
            ui.label("タイムラインで区間か点をクリックしてください。");
            return false;
        };
        let Some(keys) = self.store.state.doc.tracks.get_mut(&id) else {
            return false;
        };
        if j >= keys.len() {
            return false;
        }
        let n = keys.len();
        let mut changed = false;

        ui.label(format!(
            "ID {id} ／ {} 番目の点{}",
            j + 1,
            if j + 1 < n { " → 次の点" } else { "（最後の点）" }
        ));
        ui.horizontal(|ui| {
            ui.label("始 時間");
            changed |= ui
                .add(egui::DragValue::new(&mut keys[j].t).speed(0.01).suffix(" 秒"))
                .changed();
            ui.label("値");
            changed |= ui
                .add(egui::DragValue::new(&mut keys[j].v).speed(0.5))
                .changed();
        });
        if j + 1 < n {
            ui.horizontal(|ui| {
                ui.label("終 時間");
                changed |= ui
                    .add(egui::DragValue::new(&mut keys[j + 1].t).speed(0.01).suffix(" 秒"))
                    .changed();
                ui.label("値");
                changed |= ui
                    .add(egui::DragValue::new(&mut keys[j + 1].v).speed(0.5))
                    .changed();
            });
        }
        // 時間は隣の点を越えないようにする
        let lo = if j > 0 { keys[j - 1].t + 0.001 } else { 0.0 };
        let hi = if j + 1 < n { keys[j + 1].t - 0.001 } else { f64::INFINITY };
        if lo <= hi {
            keys[j].t = keys[j].t.clamp(lo, hi);
        }
        if j + 1 < n {
            let lo2 = keys[j].t + 0.001;
            let hi2 = if j + 2 < n { keys[j + 2].t - 0.001 } else { f64::INFINITY };
            if lo2 <= hi2 {
                keys[j + 1].t = keys[j + 1].t.clamp(lo2, hi2);
            }
        }

        if j + 1 >= n {
            ui.label("最後の点には、次の区間（イージング）がありません。");
            return changed;
        }

        // 動き方の種類
        let cur = match &keys[j].ease {
            Ease::Linear => 0,
            Ease::Hold => 1,
            Ease::Bezier(_) => 2,
        };
        let mut pick = cur;
        let names = ["直線", "階段", "ベジェ"];
        ui.horizontal(|ui| {
            ui.label("動き方");
            egui::ComboBox::from_id_salt("ease_kind")
                .selected_text(names[cur])
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        ui.selectable_value(&mut pick, i, *name);
                    }
                });
        });
        if pick != cur {
            keys[j].ease = match pick {
                0 => Ease::Linear,
                1 => Ease::Hold,
                _ => Ease::Bezier([0.42, 0.0, 0.58, 1.0]),
            };
            changed = true;
        }
        if let Ease::Bezier(p) = &mut keys[j].ease {
            ui.horizontal(|ui| {
                for (i, label) in ["x1", "y1", "x2", "y2"].iter().enumerate() {
                    let range = if i % 2 == 0 { 0.0..=1.0 } else { -1.0..=2.0 };
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p[i])
                                .speed(0.01)
                                .range(range)
                                .prefix(format!("{label} ")),
                        )
                        .changed();
                }
            });
        }

        // ---- 大きなイージング編集面 ----
        ui.add_space(6.0);
        let side = ui
            .available_width()
            .min(ui.available_height())
            .min(520.0)
            .max(140.0);
        let (resp, painter) = ui.allocate_painter(vec2(side, side), Sense::click());
        let rect = resp.rect;
        let inner = side - 2.0 * EM;
        let ex = |x: f64| rect.left() + EM + x as f32 * inner;
        let ey = |y: f64| rect.top() + EM + ((1.5 - y) / 2.0) as f32 * inner;

        let (pressed, down, ptr) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.latest_pos(),
            )
        });
        if let Ease::Bezier(p) = keys[j].ease.clone() {
            let h = [pos2(ex(p[0]), ey(p[1])), pos2(ex(p[2]), ey(p[3]))];
            if pressed {
                if let Some(pt) = ptr {
                    if rect.expand(10.0).contains(pt) {
                        for (k, hp) in h.iter().enumerate() {
                            if (pt - *hp).length() < 14.0 {
                                self.drag_h = Some(k);
                            }
                        }
                    }
                }
            }
            if let Some(k) = self.drag_h {
                if down {
                    if let Some(pt) = ptr {
                        let x = ((pt.x - rect.left() - EM) / inner) as f64;
                        let y = (1.5_f32 - (pt.y - rect.top() - EM) / inner * 2.0) as f64;
                        if let Ease::Bezier(q) = &mut keys[j].ease {
                            q[2 * k] = round3(x.clamp(0.0, 1.0));
                            q[2 * k + 1] = round3(y.clamp(-0.5, 1.5));
                            changed = true;
                        }
                    }
                } else {
                    self.drag_h = None;
                }
            }
        } else {
            self.drag_h = None;
        }

        let vis = ui.visuals().clone();
        let fg = vis.text_color();
        let sub = vis.weak_text_color();
        let acc = vis.selection.stroke.color;
        let grid = vis.widgets.noninteractive.bg_stroke.color;
        for y in [-0.5_f64, 0.0, 0.5, 1.0, 1.5] {
            let w = if y == 0.0 || y == 1.0 { 2.0 } else { 1.0 };
            painter.line_segment([pos2(ex(0.0), ey(y)), pos2(ex(1.0), ey(y))], Stroke::new(w, grid));
            painter.text(
                pos2(ex(0.0) - 5.0, ey(y)),
                Align2::RIGHT_CENTER,
                format!("{y}"),
                FontId::proportional(11.0),
                sub,
            );
        }
        for x in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            let w = if x == 0.0 || x == 1.0 { 2.0 } else { 1.0 };
            painter.line_segment([pos2(ex(x), ey(1.5)), pos2(ex(x), ey(-0.5))], Stroke::new(w, grid));
            painter.text(
                pos2(ex(x), ey(-0.5) + 6.0),
                Align2::CENTER_TOP,
                format!("{x}"),
                FontId::proportional(11.0),
                sub,
            );
        }
        let pts: Vec<Pos2> = if matches!(keys[j].ease, Ease::Hold) {
            vec![
                pos2(ex(0.0), ey(0.0)),
                pos2(ex(1.0), ey(0.0)),
                pos2(ex(1.0), ey(1.0)),
            ]
        } else {
            (0..=80)
                .map(|i| {
                    let q = i as f64 / 80.0;
                    pos2(ex(q), ey(keys[j].ease.at(q)))
                })
                .collect()
        };
        painter.add(Shape::line(pts, Stroke::new(3.0, fg)));
        if let Ease::Bezier(p) = &keys[j].ease {
            let h = [pos2(ex(p[0]), ey(p[1])), pos2(ex(p[2]), ey(p[3]))];
            painter.line_segment([pos2(ex(0.0), ey(0.0)), h[0]], Stroke::new(1.5, acc));
            painter.line_segment([pos2(ex(1.0), ey(1.0)), h[1]], Stroke::new(1.5, acc));
            for hp in h {
                painter.rect_filled(Rect::from_center_size(hp, vec2(12.0, 12.0)), 0.0, acc);
            }
        }
        changed
    }
}

impl eframe::App for EasingApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(if self.pending {
            120
        } else {
            250
        }));
        // 編集中は、他のプラグインの変更で上書きされないよう読み直さない
        if self.drag_h.is_none() && !self.pending {
            self.store.poll();
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if self.editor(ui) {
                self.pending = true;
            }
        });
        if self.pending && self.last_save.elapsed() > Duration::from_millis(150) {
            self.flush();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill.to_normalized_gamma_f32()
    }
}
