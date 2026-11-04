use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotBounds, Points};
use serde::{Deserialize, Serialize};

use crate::model::{self, Value};

const MAX_SERIES: usize = 16;
const MAX_SAMPLES: usize = 12_000;
const HISTORY_SECONDS: f64 = 600.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Series {
    pub path: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub open: bool,
    pub title: String,
    pub seconds: f64,
    pub series: Vec<Series>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: true,
            title: "Plot".into(),
            seconds: 30.0,
            series: Vec::new(),
        }
    }
}

impl Settings {
    pub fn add(&mut self, path: &str) {
        if self.series.iter().all(|s| s.path != path) && self.series.len() < MAX_SERIES {
            self.series.push(Series {
                path: path.into(),
                hidden: false,
            });
        }
        self.open = true;
    }
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    time: f64,
    value: Option<f64>,
}

#[derive(Default)]
struct History {
    values: BTreeMap<String, VecDeque<Sample>>,
}

impl History {
    fn push(&mut self, path: &str, time: f64, value: Option<f64>, expected_interval: f64) {
        let samples = self.values.entry(path.into()).or_default();
        if let Some(last) = samples.back()
            && time - last.time > expected_interval * 2.5
        {
            samples.push_back(Sample { time, value: None });
        }
        samples.push_back(Sample { time, value });
        while samples.len() > MAX_SAMPLES
            || samples
                .front()
                .is_some_and(|s| time - s.time > HISTORY_SECONDS)
        {
            samples.pop_front();
        }
    }
}

pub fn number(value: &Value) -> Option<f64> {
    if value.error.is_some() || value.expandable || !value.children.is_empty() {
        return None;
    }
    match value.raw().as_str() {
        "true" => Some(1.0),
        "false" => Some(0.0),
        raw => raw.parse::<f64>().ok().filter(|n| n.is_finite()),
    }
}

pub fn numeric_values<'a>(values: &'a [Value], out: &mut BTreeMap<String, &'a Value>) {
    for value in values {
        if number(value).is_some() {
            out.entry(value.path.clone()).or_insert(value);
        }
        numeric_values(&value.children, out);
    }
}

pub struct DraggedValue(pub String);

pub struct Graph {
    origin: Instant,
    history: History,
    follow: bool,
    latest: f64,
    fit: bool,
    editing_title: Option<String>,
    title_focus: bool,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            history: History::default(),
            follow: true,
            latest: 0.0,
            fit: true,
            editing_title: None,
            title_focus: false,
        }
    }
}

impl Graph {
    pub fn sample(
        &mut self,
        settings: &Settings,
        declared: &[Value],
        watched: &[(String, Vec<Value>)],
        interval_ms: u64,
    ) {
        let time = self.origin.elapsed().as_secs_f64();
        self.latest = time;
        self.history
            .values
            .retain(|path, _| settings.series.iter().any(|s| &s.path == path));
        for series in &settings.series {
            let value = model::find(declared, &series.path)
                .and_then(number)
                .or_else(|| {
                    watched
                        .iter()
                        .find_map(|(_, values)| model::find(values, &series.path).and_then(number))
                });
            self.history
                .push(&series.path, time, value, interval_ms as f64 / 1000.0);
        }
    }

    pub fn gap(&mut self) {
        let time = self.origin.elapsed().as_secs_f64();
        for samples in self.history.values.values_mut() {
            if samples.back().is_some_and(|s| s.value.is_some()) {
                samples.push_back(Sample { time, value: None });
            }
        }
    }

    /// Returns true when series membership changes and polling must be reconfigured.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        available: &BTreeMap<String, &Value>,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            if self.editing_title.is_some() {
                let response =
                    ui.add(egui::TextEdit::singleline(&mut settings.title).desired_width(200.0));
                if self.title_focus {
                    response.request_focus();
                    self.title_focus = false;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if let Some(original) = self.editing_title.take() {
                        settings.title = original;
                    }
                } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) || response.lost_focus() {
                    self.editing_title = None;
                    if settings.title.trim().is_empty() {
                        settings.title = "Plot".into();
                    }
                }
            } else if ui
                .add(
                    egui::Label::new(RichText::new(&settings.title).heading())
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Rename plot")
                .clicked()
            {
                self.editing_title = Some(settings.title.clone());
                self.title_focus = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Hide plot").clicked() {
                    settings.open = false;
                }
                ui.menu_button("+", |ui| {
                    ui.set_max_height(300.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let mut offered = false;
                        for (path, value) in available {
                            if settings.series.iter().any(|s| s.path == *path) {
                                continue;
                            }
                            offered = true;
                            if ui
                                .add_enabled(
                                    settings.series.len() < MAX_SERIES,
                                    egui::Button::new(path),
                                )
                                .on_hover_text(&value.type_name)
                                .clicked()
                            {
                                settings.add(path);
                                changed = true;
                                self.fit = true;
                                ui.close();
                            }
                        }
                        if !offered {
                            ui.weak("Add a numeric watch first");
                        }
                        if settings.series.len() >= MAX_SERIES {
                            ui.weak("16 values maximum");
                        }
                    });
                })
                .response
                .on_hover_text("Add value to plot");
            });
        });
        let bottom = 30.0;
        let height = (ui.available_height() - bottom).max(160.0);
        let mut remove = None;
        let mut visibility_changed = false;
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(150.0, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(height)
                        .show(ui, |ui| {
                            if settings.series.is_empty() {
                                ui.weak("Use + to add values");
                            }
                            for series in &mut settings.series {
                                ui.horizontal(|ui| {
                                    let color = if series.hidden {
                                        ui.visuals().weak_text_color()
                                    } else {
                                        color(&series.path)
                                    };
                                    let (swatch, _) = ui.allocate_exact_size(
                                        egui::vec2(6.0, 6.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(swatch, 1.0, color);
                                    let label = short(&series.path, 17);
                                    if ui
                                        .add(
                                            egui::Label::new(RichText::new(label).color(color))
                                                .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(format!(
                                            "{}\nClick to hide/show",
                                            series.path
                                        ))
                                        .clicked()
                                    {
                                        series.hidden = !series.hidden;
                                        visibility_changed = true;
                                    }
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("Remove from plot")
                                        .clicked()
                                    {
                                        remove = Some(series.path.clone());
                                    }
                                });
                            }
                        });
                },
            );
            let response = Plot::new("live-plot")
                .id(egui::Id::new("live-plot"))
                .height(height)
                .width(ui.available_width())
                .min_size(egui::vec2(150.0, 160.0))
                .x_axis_label("Time (s)")
                .allow_axis_zoom_drag(true)
                .allow_zoom(true)
                .allow_drag(true)
                .allow_scroll(true)
                .allow_double_click_reset(false)
                .show(ui, |plot| {
                    if self.follow || self.fit {
                        let end = self.latest.max(settings.seconds);
                        let start = end - settings.seconds;
                        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
                        for series in settings.series.iter().filter(|s| !s.hidden) {
                            if let Some(samples) = self.history.values.get(&series.path) {
                                for sample in
                                    samples.iter().filter(|s| s.time >= start && s.time <= end)
                                {
                                    if let Some(y) = sample.value {
                                        min = min.min(y);
                                        max = max.max(y);
                                    }
                                }
                            }
                        }
                        if !min.is_finite() {
                            min = -1.0;
                            max = 1.0;
                        }
                        let pad = ((max - min) * 0.08)
                            .max(max.abs().max(min.abs()) * 0.01)
                            .max(0.001);
                        plot.set_plot_bounds(PlotBounds::from_min_max(
                            [start, min - pad],
                            [end, max + pad],
                        ));
                    }
                    for series in settings.series.iter().filter(|s| !s.hidden) {
                        let Some(samples) = self.history.values.get(&series.path) else {
                            continue;
                        };
                        let mut segment = Vec::new();
                        let mut draw = |segment: &mut Vec<[f64; 2]>| {
                            if segment.len() == 1 {
                                plot.points(
                                    Points::new(&series.path, std::mem::take(segment))
                                        .color(color(&series.path))
                                        .radius(2.5_f32),
                                );
                            } else if !segment.is_empty() {
                                plot.line(
                                    Line::new(&series.path, std::mem::take(segment))
                                        .color(color(&series.path))
                                        .width(1.5_f32),
                                );
                            }
                        };
                        for sample in samples {
                            if let Some(y) = sample.value {
                                segment.push([sample.time, y]);
                            } else {
                                draw(&mut segment);
                            }
                        }
                        draw(&mut segment);
                    }
                });
            if let Some(value) = response.response.dnd_release_payload::<DraggedValue>() {
                settings.add(&value.0);
                changed = true;
                self.fit = true;
            } else {
                self.fit = false;
            }
            let interacted = response.response.dragged()
                || (response.response.hovered()
                    && ui.input(|i| {
                        i.smooth_scroll_delta != egui::Vec2::ZERO || i.zoom_delta() != 1.0
                    }))
                || ui.input(|i| {
                    i.pointer.any_down()
                        && response
                            .response
                            .rect
                            .expand(35.0)
                            .contains(i.pointer.hover_pos().unwrap_or_default())
                });
            if interacted {
                self.follow = false;
            }
            if response.response.double_clicked() {
                self.follow = true;
                self.fit = true;
            }
        });
        if let Some(path) = remove {
            settings.series.retain(|s| s.path != path);
            self.history.values.remove(&path);
            changed = true;
            self.fit = true;
        }
        if visibility_changed {
            self.fit = self.follow;
        }
        ui.horizontal(|ui| {
            if ui.selectable_label(self.follow, "Live").on_hover_text("Follow latest samples and fit both axes").clicked() { self.follow = true; self.fit = true; }
            if ui.selectable_label(!self.follow, "Freeze").on_hover_text("Hold the view; acquisition continues").clicked() { self.follow = false; }
            if ui.add(egui::DragValue::new(&mut settings.seconds).range(1.0..=600.0).suffix(" s")).on_hover_text("Visible time window").changed() { self.fit = true; }
            if ui.small_button("Fit").clicked() { self.fit = true; }
            if ui.small_button("Clear").clicked() { self.history.values.clear(); self.fit = true; }
            ui.weak("Drag to pan · drag axes to stretch").on_hover_text("Scroll/pinch to zoom. Right-drag a box to zoom. Double-click to resume Live. History: 10 minutes, at most 12,000 samples per value. Frozen history still expires.");
        });
        changed
    }
}

fn color(path: &str) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(114, 187, 255),
        Color32::from_rgb(255, 183, 104),
        Color32::from_rgb(113, 215, 166),
        Color32::from_rgb(207, 154, 255),
        Color32::from_rgb(255, 133, 151),
        Color32::from_rgb(112, 216, 224),
        Color32::from_rgb(220, 218, 118),
        Color32::from_rgb(220, 165, 203),
    ];
    let hash = path.bytes().fold(0_usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    });
    COLORS[hash % COLORS.len()]
}

fn short(path: &str, count: usize) -> String {
    let n = path.chars().count();
    if n <= count {
        path.into()
    } else {
        format!("…{}", path.chars().skip(n - count + 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_bounds_memory_and_leaves_gaps_for_missed_reads() {
        let mut history = History::default();
        history.push("x", 0.0, Some(1.0), 0.5);
        history.push("x", 2.0, Some(2.0), 0.5);
        assert!(history.values["x"].iter().any(|s| s.value.is_none()));
        for i in 0..20_000 {
            history.push("x", i as f64 * 0.02 + 3.0, Some(0.0), 0.02);
        }
        assert_eq!(history.values["x"].len(), MAX_SAMPLES);
        history.push("x", 2000.0, Some(3.0), 0.5);
        assert!(history.values["x"].iter().all(|s| s.time >= 1400.0));
    }

    #[test]
    fn finite_scalar_only_and_hidden_series_keep_sampling() {
        let value = |display: &str| {
            model::parse_values(&serde_json::json!({"version":1,"values":[{"path":"x","type":"f32","kind":"watch","writable":false,"display":display}]}).to_string()).unwrap().remove(0)
        };
        assert_eq!(number(&value("x = true")), Some(1.0));
        assert_eq!(number(&value("x = 0.25 rad")), Some(0.25));
        assert_eq!(number(&value("x = NaN")), None);
        assert_eq!(number(&value("x = 0x20000000")), None);
        let mut settings = Settings::default();
        settings.add("x");
        settings.add("x");
        settings.series[0].hidden = true;
        let mut graph = Graph::default();
        graph.sample(&settings, &[value("x = 2")], &[], 500);
        graph.sample(&settings, &[], &[], 500);
        assert_eq!(settings.series.len(), 1);
        assert_eq!(graph.history.values["x"][0].value, Some(2.0));
        assert_eq!(graph.history.values["x"][1].value, None);
    }
    fn frame(
        ctx: &egui::Context,
        graph: &mut Graph,
        settings: &mut Settings,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let mut source = egui::Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let label =
                        ui.add(egui::Label::new("source").sense(egui::Sense::click_and_drag()));
                    source = label.rect;
                    label.dnd_set_drag_payload(DraggedValue("x".into()));
                    graph.show(ui, settings, &BTreeMap::new());
                });
            },
        );
        source
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn memory(ctx: &egui::Context) -> egui_plot::PlotMemory {
        egui_plot::PlotMemory::load(ctx, egui::Id::new("live-plot")).unwrap()
    }

    #[test]
    fn dragging_a_watch_adds_it_to_the_plot() {
        let ctx = egui::Context::default();
        let mut graph = Graph::default();
        let mut settings = Settings::default();
        let source = frame(&ctx, &mut graph, &mut settings, vec![]);
        frame(&ctx, &mut graph, &mut settings, vec![]);
        let start = source.center();
        let end = memory(&ctx).transform().frame().center();
        frame(
            &ctx,
            &mut graph,
            &mut settings,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
        );
        frame(
            &ctx,
            &mut graph,
            &mut settings,
            vec![egui::Event::PointerMoved(start + egui::vec2(15.0, 0.0))],
        );
        frame(
            &ctx,
            &mut graph,
            &mut settings,
            vec![egui::Event::PointerMoved(end)],
        );
        frame(&ctx, &mut graph, &mut settings, vec![pointer(end, false)]);
        assert_eq!(settings.series.first().map(|s| s.path.as_str()), Some("x"));
    }

    #[test]
    fn stretching_each_axis_changes_only_that_axis_and_holds_the_view() {
        for axis in 0..2 {
            let ctx = egui::Context::default();
            let mut graph = Graph::default();
            let mut settings = Settings::default();
            frame(&ctx, &mut graph, &mut settings, vec![]);
            frame(&ctx, &mut graph, &mut settings, vec![]);
            let before = memory(&ctx);
            let rect = *before.transform().frame();
            let start = if axis == 0 {
                egui::pos2(rect.center().x, rect.bottom() + 10.0)
            } else {
                egui::pos2(rect.left() - 10.0, rect.center().y)
            };
            let delta = if axis == 0 {
                egui::vec2(25.0, 0.0)
            } else {
                egui::vec2(0.0, 25.0)
            };
            frame(
                &ctx,
                &mut graph,
                &mut settings,
                vec![egui::Event::PointerMoved(start), pointer(start, true)],
            );
            frame(
                &ctx,
                &mut graph,
                &mut settings,
                vec![egui::Event::PointerMoved(start + delta)],
            );
            frame(
                &ctx,
                &mut graph,
                &mut settings,
                vec![pointer(start + delta, false)],
            );
            let after = *memory(&ctx).bounds();
            if axis == 0 {
                assert_ne!(
                    before.bounds().width(),
                    after.width(),
                    "horizontal axis stretch"
                );
                assert_eq!(before.bounds().height(), after.height());
            } else {
                assert_ne!(
                    before.bounds().height(),
                    after.height(),
                    "vertical axis stretch"
                );
                assert_eq!(before.bounds().width(), after.width());
            }
            assert!(!graph.follow, "axis stretching freezes the view");
            graph.latest += 20.0;
            frame(&ctx, &mut graph, &mut settings, vec![]);
            assert_eq!(
                *memory(&ctx).bounds(),
                after,
                "new samples must not reset the selected view"
            );
        }
    }
}
