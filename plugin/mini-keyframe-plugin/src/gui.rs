use aviutl2_eframe::{eframe, egui};
use egui::{pos2, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke};
use mini_keyframe_core::{eval_keys, Doc, Ease, Key, Selection, Store};
use std::time::{Duration, Instant};

const LM: f32 = 60.0; // 左のラベル欄の幅
const RM: f32 = 14.0;
const TOP: f32 = 24.0; // 時間目盛りの高さ
const RH: f32 = 64.0; // 1行の高さ

/// 時間と画面のx座標の対応。
#[derive(Clone, Copy)]
struct View {
    l: f32,
    w: f32,
    t0: f64,
    span: f64,
}
impl View {
    fn x(&self, t: f64) -> f32 {
        self.l + ((t - self.t0) / self.span) as f32 * self.w
    }
    fn t(&self, x: f32) -> f64 {
        self.t0 + ((x - self.l) / self.w) as f64 * self.span
    }
}

enum Hit {
    Key(u32, usize),
    Seg(u32, usize),
    Row(u32),
    None,
}

/// Ctrl+Z → Some(true)（元に戻す）、Ctrl+Shift+Z / Ctrl+Y → Some(false)（やり直し）
fn undo_shortcut(ui: &egui::Ui) -> Option<bool> {
    ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
        {
            Some(false)
        } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
            Some(true)
        } else {
            None
        }
    })
}

fn nice_step(range: f64, n: f64) -> f64 {
    let raw = range / n;
    let p = 10f64.powf(raw.log10().floor());
    let f = raw / p;
    let m = if f < 1.5 {
        1.0
    } else if f < 3.5 {
        2.0
    } else if f < 7.5 {
        5.0
    } else {
        10.0
    };
    m * p
}

pub(crate) struct TimelineApp {
    store: Store,
    total: f64,
    t0: f64,
    span: f64,
    follow_full: bool,
    snap: bool,
    fps: Option<f64>,
    drag: Option<(u32, usize)>,
    drag_before: Option<Doc>,
    /// 再生位置（選択オブジェクトの先頭からの秒数）
    playhead: Option<f64>,
    obj_start: Option<usize>,
    obj_checked: Instant,
    last_frame: Option<usize>,
    fast_until: Instant,
    dirty: bool,
    msg: String,
}

impl TimelineApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        Self {
            store: Store::open(),
            total: 0.0,
            t0: 0.0,
            span: 1.0,
            follow_full: true,
            snap: true,
            fps: None,
            drag: None,
            drag_before: None,
            playhead: None,
            obj_start: None,
            obj_checked: Instant::now(),
            last_frame: None,
            fast_until: Instant::now(),
            dirty: false,
            msg: String::new(),
        }
    }

    fn save(&mut self) {
        let lua = crate::LUA_PATH.get().map(|p| p.as_path());
        match self.store.save(lua) {
            Ok(()) => self.msg.clear(),
            Err(e) => self.msg = format!("保存に失敗: {e}"),
        }
    }

    fn history(&mut self, undo: bool) {
        let ok = if undo { self.store.undo() } else { self.store.redo() };
        if ok {
            self.drag = None;
            self.drag_before = None;
            self.save();
        }
    }

    fn select(&mut self, id: u32, key: Option<usize>) {
        self.store.state.sel = Selection { id: Some(id), key };
        self.save();
    }

    fn total_len(&self) -> f64 {
        let m = self
            .store
            .state
            .doc
            .tracks
            .values()
            .flat_map(|k| k.iter().map(|k| k.t))
            .fold(1.0, f64::max);
        self.total.max(m * 1.05).max(1.0)
    }

    fn clamp_view(&mut self, total: f64) {
        self.span = self.span.clamp(total.min(0.2), total);
        self.t0 = self.t0.clamp(0.0, (total - self.span).max(0.0));
    }

    fn zoom_center(&mut self, f: f64) {
        let total = self.total_len();
        let c = self.t0 + self.span / 2.0;
        self.follow_full = false;
        self.span *= f;
        self.clamp_view(total);
        self.t0 = c - self.span / 2.0;
        self.clamp_view(total);
    }

    fn snap_t(&self, t: f64) -> f64 {
        let t = t.max(0.0);
        let t = match (self.snap, self.fps) {
            (true, Some(f)) => (t * f).round() / f,
            _ => t,
        };
        (t * 1e4).round() / 1e4
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!self.store.state.undo.is_empty(), egui::Button::new("↶"))
                    .on_hover_text("元に戻す (Ctrl+Z)")
                    .clicked()
                {
                    self.history(true);
                }
                if ui
                    .add_enabled(!self.store.state.redo.is_empty(), egui::Button::new("↷"))
                    .on_hover_text("やり直し (Ctrl+Y)")
                    .clicked()
                {
                    self.history(false);
                }
                ui.separator();
                if ui.button("＋ID").clicked() {
                    let before = self.store.state.doc.clone();
                    let id = self.store.state.doc.next_id();
                    self.store.state.doc.tracks.insert(
                        id,
                        vec![
                            Key { t: 0.0, v: 0.0, ease: Ease::Linear },
                            Key { t: 1.0, v: 100.0, ease: Ease::Linear },
                        ],
                    );
                    self.store.record(before, "");
                    self.select(id, None);
                }
                if ui.button("選択中のIDを削除").clicked() {
                    if let Some(id) = self.store.state.sel.id {
                        if self.store.state.doc.tracks.len() > 1 {
                            let before = self.store.state.doc.clone();
                            self.store.state.doc.tracks.remove(&id);
                            self.store.record(before, "");
                            self.store.state.sel = Selection::default();
                            self.save();
                        }
                    }
                }
                ui.separator();
                ui.label("全体の長さ");
                if ui
                    .add(
                        egui::DragValue::new(&mut self.total)
                            .speed(0.1)
                            .range(0.0..=3600.0)
                            .suffix(" 秒"),
                    )
                    .changed()
                {
                    self.follow_full = true;
                }
                if ui.button("選択オブジェクトの長さに合わせる").clicked() {
                    match crate::object_length_sec() {
                        Ok(s) => {
                            self.total = s;
                            self.follow_full = true;
                            self.msg.clear();
                        }
                        Err(e) => self.msg = e,
                    }
                }
                if ui.button("－").clicked() {
                    self.zoom_center(1.6);
                }
                if ui.button("＋").clicked() {
                    self.zoom_center(0.6);
                }
                if ui.button("全体").clicked() {
                    self.follow_full = true;
                }
                ui.checkbox(&mut self.snap, "フレームに吸着");
            });
            if !self.msg.is_empty() {
                ui.label(egui::RichText::new(&self.msg).color(Color32::from_rgb(230, 120, 60)));
            }
        });
    }

    fn hit(&self, ids: &[u32], rect: Rect, v: View, p: Pos2) -> Hit {
        let i = ((p.y - rect.top() - TOP) / RH).floor();
        if i < 0.0 || i as usize >= ids.len() {
            return Hit::None;
        }
        let id = ids[i as usize];
        if p.x < v.l {
            return Hit::Row(id);
        }
        let mid = rect.top() + TOP + i * RH + RH / 2.0;
        let keys = &self.store.state.doc.tracks[&id];
        if let Some(j) = keys
            .iter()
            .position(|k| (v.x(k.t) - p.x).abs() < 9.0 && (mid - p.y).abs() < 12.0)
        {
            return Hit::Key(id, j);
        }
        let t = v.t(p.x);
        if let Some(j) = (0..keys.len().saturating_sub(1)).find(|&j| keys[j].t <= t && t < keys[j + 1].t)
        {
            return Hit::Seg(id, j);
        }
        Hit::Row(id)
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let ids: Vec<u32> = self.store.state.doc.tracks.keys().copied().collect();
        let total = self.total_len();
        if self.follow_full {
            self.span = total;
            self.t0 = 0.0;
        }
        self.clamp_view(total);

        let avail = ui.available_width().max(200.0);
        let height = TOP + RH * ids.len().max(1) as f32 + 6.0;
        let (resp, painter) = ui.allocate_painter(egui::vec2(avail, height), egui::Sense::click());
        let rect = resp.rect;
        let (l, w) = (rect.left() + LM, (rect.width() - LM - RM).max(10.0));

        let (pressed, down, released, ptr, scroll, ctrl) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.latest_pos(),
                i.smooth_scroll_delta,
                i.modifiers.command,
            )
        });
        let inside = ptr.is_some_and(|p| rect.contains(p));

        // ホイール：横スクロール ／ Ctrl+ホイール：拡大縮小
        if inside && (scroll.x != 0.0 || scroll.y != 0.0) {
            self.follow_full = false;
            if ctrl {
                let px = ptr.map(|p| p.x).unwrap_or(l);
                let t_at = self.t0 + ((px - l) / w) as f64 * self.span;
                self.span *= (-(scroll.y as f64) * 0.01).exp();
                self.clamp_view(total);
                self.t0 = t_at - ((px - l) / w) as f64 * self.span;
            } else {
                self.t0 -= (scroll.x + scroll.y) as f64 / w as f64 * self.span;
            }
            self.clamp_view(total);
        }
        let view = View { l, w, t0: self.t0, span: self.span };

        // クリック・ドラッグ
        if pressed && inside {
            if let Some(p) = ptr {
                match self.hit(&ids, rect, view, p) {
                    Hit::Key(id, j) => {
                        self.select(id, Some(j));
                        self.drag_before = Some(self.store.state.doc.clone());
                        self.drag = Some((id, j));
                    }
                    Hit::Seg(id, j) => self.select(id, Some(j)),
                    Hit::Row(id) => self.select(id, None),
                    Hit::None => {}
                }
            }
        }
        if let Some((id, j)) = self.drag {
            if down {
                if let Some(p) = ptr {
                    let t = self.snap_t(view.t(p.x));
                    let mut moved_before: Option<Doc> = None;
                    if let Some(keys) = self.store.state.doc.tracks.get_mut(&id) {
                        let lo = if j > 0 { keys[j - 1].t + 0.001 } else { 0.0 };
                        let hi = if j + 1 < keys.len() { keys[j + 1].t - 0.001 } else { f64::INFINITY };
                        if lo <= hi && j < keys.len() {
                            let nt = t.clamp(lo, hi);
                            if (keys[j].t - nt).abs() > 1e-9 {
                                moved_before = self.drag_before.take();
                                keys[j].t = nt;
                                self.dirty = true;
                            }
                        }
                    }
                    if let Some(b) = moved_before {
                        self.store.record(b, "");
                    }
                }
            }
            if released || !down {
                self.drag = None;
                self.drag_before = None;
                if self.dirty {
                    self.dirty = false;
                    self.save();
                }
            }
        }
        if resp.double_clicked() {
            if let Some(p) = ptr {
                match self.hit(&ids, rect, view, p) {
                    Hit::Key(id, j) => {
                        let before = self.store.state.doc.clone();
                        let mut removed = false;
                        if let Some(keys) = self.store.state.doc.tracks.get_mut(&id) {
                            if keys.len() > 1 {
                                keys.remove(j);
                                removed = true;
                            }
                        }
                        if removed {
                            self.store.record(before, "");
                        }
                        self.store.state.sel = Selection { id: Some(id), key: None };
                        self.save();
                    }
                    Hit::Seg(id, _) | Hit::Row(id) if p.x >= view.l => {
                        let t = self.snap_t(view.t(p.x));
                        let before = self.store.state.doc.clone();
                        let mut new_index = None;
                        if let Some(keys) = self.store.state.doc.tracks.get_mut(&id) {
                            if !keys.iter().any(|k| (k.t - t).abs() < 1e-6) {
                                let v = eval_keys(keys, t).unwrap_or(0.0);
                                let pos = keys.iter().position(|k| k.t > t).unwrap_or(keys.len());
                                keys.insert(pos, Key { t, v: (v * 100.0).round() / 100.0, ease: Ease::Linear });
                                new_index = Some(pos);
                            }
                        }
                        if let Some(pos) = new_index {
                            self.store.record(before, "");
                            self.select(id, Some(pos));
                        }
                    }
                    _ => {}
                }
            }
        }

        // ---- 描画 ----
        let vis = ui.visuals().clone();
        let fg = vis.text_color();
        let sub = vis.weak_text_color();
        let acc = vis.selection.stroke.color;
        let grid = vis.widgets.noninteractive.bg_stroke.color;
        let sel = self.store.state.sel.clone();

        let st = nice_step(view.span, 8.0);
        let mut t = (view.t0 / st).ceil() * st;
        while t <= view.t0 + view.span + 1e-9 {
            let x = view.x(t);
            painter.line_segment(
                [pos2(x, rect.top() + TOP - 4.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, grid),
            );
            painter.text(
                pos2(x, rect.top() + 2.0),
                Align2::CENTER_TOP,
                format!("{}s", (t * 100.0).round() / 100.0),
                FontId::proportional(11.0),
                sub,
            );
            t += st;
        }

        for (i, id) in ids.iter().enumerate() {
            let y0 = rect.top() + TOP + i as f32 * RH;
            let (mid, top, bot) = (y0 + RH / 2.0, y0 + 10.0, y0 + RH - 10.0);
            let active = sel.id == Some(*id);
            if active {
                painter.rect_filled(
                    Rect::from_min_max(pos2(rect.left(), y0), pos2(rect.right(), y0 + RH)),
                    0.0,
                    vis.faint_bg_color,
                );
            }
            painter.text(
                pos2(rect.left() + 8.0, mid),
                Align2::LEFT_CENTER,
                format!("ID {id}"),
                FontId::proportional(13.0),
                if active { acc } else { fg },
            );
            painter.line_segment(
                [pos2(rect.left(), y0 + RH), pos2(rect.right(), y0 + RH)],
                Stroke::new(1.0, grid),
            );
            let clip = painter.with_clip_rect(Rect::from_min_max(
                pos2(view.l - 8.0, y0),
                pos2(view.l + view.w + 8.0, y0 + RH),
            ));
            let keys = &self.store.state.doc.tracks[id];
            for j in 0..keys.len().saturating_sub(1) {
                let (a, b) = (&keys[j], &keys[j + 1]);
                let (x0, x1) = (view.x(a.t), view.x(b.t));
                if x1 < view.l - 10.0 || x0 > view.l + view.w + 10.0 {
                    continue;
                }
                let is_sel = active && sel.key == Some(j);
                let col = if is_sel { acc } else { fg };
                clip.rect_filled(
                    Rect::from_min_max(pos2(x0, top), pos2(x1, bot)),
                    0.0,
                    col.gamma_multiply(if is_sel { 0.22 } else { 0.08 }),
                );
                let dv = b.v - a.v;
                let pts: Vec<Pos2> = (0..=24)
                    .map(|n| {
                        let q = n as f64 / 24.0;
                        let e = a.ease.at(q) as f32;
                        let x = x0 + (x1 - x0) * q as f32;
                        let y = if dv == 0.0 {
                            mid
                        } else if dv > 0.0 {
                            bot - (bot - top) * e
                        } else {
                            top + (bot - top) * e
                        };
                        pos2(x, y)
                    })
                    .collect();
                clip.add(Shape::line(pts, Stroke::new(if is_sel { 2.5 } else { 1.5 }, col)));
            }
            for (j, k) in keys.iter().enumerate() {
                let x = view.x(k.t);
                if x < view.l - 10.0 || x > view.l + view.w + 10.0 {
                    continue;
                }
                let s = active && sel.key == Some(j);
                let pts = vec![
                    pos2(x, mid - 7.0),
                    pos2(x + 7.0, mid),
                    pos2(x, mid + 7.0),
                    pos2(x - 7.0, mid),
                ];
                clip.text(
                    pos2(x, mid - 9.0),
                    Align2::CENTER_BOTTOM,
                    format!("{}", (k.v * 100.0).round() / 100.0),
                    FontId::proportional(10.0),
                    sub,
                );
                clip.add(Shape::convex_polygon(
                    pts,
                    if s { acc } else { vis.window_fill },
                    Stroke::new(2.0, if s { acc } else { fg }),
                ));
            }
        }
        // 再生位置の縦線
        if let Some(ph) = self.playhead {
            let x = view.x(ph);
            if x >= view.l && x <= view.l + view.w {
                let red = Color32::from_rgb(235, 80, 80);
                painter.line_segment(
                    [pos2(x, rect.top() + TOP - 4.0), pos2(x, rect.bottom())],
                    Stroke::new(2.0, red),
                );
                painter.text(
                    pos2(x + 4.0, rect.top() + TOP - 4.0),
                    Align2::LEFT_BOTTOM,
                    format!("{:.2}s", ph),
                    FontId::proportional(10.0),
                    red,
                );
            }
        }
    }
}

impl eframe::App for TimelineApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // イージングパネル側の変更を拾うため、定期的に再描画してファイルを確認する
        // 再生位置を取得する（フレームが動いている間は、細かく再描画する）
        if let Some((frame, fps)) = crate::cursor_frame() {
            if self.last_frame != Some(frame) {
                self.last_frame = Some(frame);
                self.fast_until = Instant::now() + Duration::from_millis(500);
            }
            if self.obj_checked.elapsed() > Duration::from_millis(300) {
                self.obj_start = crate::focused_object_start();
                self.obj_checked = Instant::now();
            }
            self.playhead = self.obj_start.map(|s| (frame as f64 - s as f64) / fps);
        }
        let wait = if Instant::now() < self.fast_until { 33 } else { 200 };
        ui.ctx().request_repaint_after(Duration::from_millis(wait));
        if let Some(is_undo) = undo_shortcut(ui) {
            self.history(is_undo);
        }
        if self.fps.is_none() {
            self.fps = crate::scene_fps();
        }
        if self.drag.is_none() {
            self.store.poll();
        }
        self.toolbar(ui);
        egui::CentralPanel::default().show(ui, |ui| self.timeline(ui));
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill.to_normalized_gamma_f32()
    }
}
