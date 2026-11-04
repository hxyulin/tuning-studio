use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use crate::backend::{Backend, Event, Work};
use crate::model::{self, Saved, Suggestion, Value};

struct Edit {
    path: String,
    text: String,
    focus: bool,
    pending: bool,
    error: Option<String>,
}

pub struct App {
    saved: Saved,
    backend: Backend,
    connected: bool,
    connecting: bool,
    owned: bool,
    status: String,
    notice: Option<String>,
    replacing: Option<String>,
    declared: Vec<Value>,
    watched: Vec<(String, Vec<Value>)>,
    read_error: Option<String>,
    probes: Vec<(String, String)>,
    add: String,
    suggestions: Vec<Suggestion>,
    suggestion_error: Option<String>,
    selected_suggestion: usize,
    suggestion_due: Option<Instant>,
    edit: Option<Edit>,
    selected: Option<String>,
    reason: Option<Value>,
    pending_confirm: BTreeSet<String>,
    paused: bool,
    logs: VecDeque<String>,
    last_sample: Option<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, options: crate::Options) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut saved: Saved = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "session"))
            .unwrap_or_default();
        if let Some(elf) = options.elf {
            saved.elf = elf;
        }
        if let Some(probe) = options.probe {
            saved.probe = probe;
        }
        if let Some(chip) = options.chip {
            saved.chip = chip;
        }
        if let Some(cli) = options.cli {
            saved.cli = cli;
        }
        let backend = Backend::new(cc.egui_ctx.clone());
        let _ = backend.tx.send(Work::Configure(saved.clone()));
        let _ = backend.tx.send(Work::List(saved.cli.clone()));
        if options.connect {
            let _ = backend.tx.send(Work::Connect(saved.clone()));
        }
        Self {
            saved,
            backend,
            connected: false,
            connecting: options.connect,
            owned: false,
            status: "Disconnected".into(),
            notice: None,
            replacing: None,
            declared: vec![],
            watched: vec![],
            read_error: None,
            probes: vec![],
            add: String::new(),
            suggestions: vec![],
            suggestion_error: None,
            selected_suggestion: 0,
            suggestion_due: None,
            edit: None,
            selected: None,
            reason: None,
            pending_confirm: BTreeSet::new(),
            paused: false,
            logs: VecDeque::new(),
            last_sample: None,
        }
    }

    fn send(&self, work: Work) {
        let _ = self.backend.tx.send(work);
    }
    fn configure(&self) {
        self.send(Work::Configure(self.saved.clone()));
    }

    fn events(&mut self) {
        while let Ok(event) = self.backend.rx.try_recv() {
            match event {
                Event::State {
                    connected,
                    owned,
                    status,
                } => {
                    self.connected = connected;
                    self.owned = owned;
                    self.connecting = status == "Connecting…";
                    self.status = status;
                    if !connected {
                        self.declared.clear();
                        self.watched.clear();
                        self.edit = None;
                    }
                }
                Event::Sample {
                    declared,
                    watched,
                    error,
                } => {
                    self.declared = declared;
                    self.watched = watched;
                    self.read_error = error;
                    self.last_sample = Some(Instant::now());
                    self.pending_confirm.clear();
                }
                Event::Suggestions {
                    query,
                    items,
                    error,
                } => {
                    if query == self.add {
                        self.suggestions = items;
                        self.suggestion_error = error;
                        self.selected_suggestion = self
                            .selected_suggestion
                            .min(self.suggestions.len().saturating_sub(1));
                    }
                }
                Event::Written { path, result } => match result {
                    Ok(reply) => {
                        self.logs.push_back(reply.trim().into());
                        self.pending_confirm.insert(path.clone());
                        if self.edit.as_ref().is_some_and(|e| e.path == path) {
                            self.edit = None;
                        }
                    }
                    Err(error) => {
                        if let Some(edit) = &mut self.edit
                            && edit.path == path
                        {
                            edit.pending = false;
                            edit.focus = true;
                            edit.error = Some(error);
                        }
                    }
                },
                Event::Probes(probes) => self.probes = probes,
                Event::Notice(text) => {
                    self.notice = Some(text);
                    self.connecting = false;
                }
                Event::Log(text) => self.logs.push_back(text),
            }
            while self.logs.len() > 200 {
                self.logs.pop_front();
            }
        }
    }

    fn top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong("Tuning Studio");
                ui.separator();
                if self.connected {
                    let color = if self.status.starts_with("halted") {
                        Color32::from_rgb(235, 190, 105)
                    } else {
                        Color32::from_rgb(104, 201, 154)
                    };
                    ui.label(
                        RichText::new(if self.status.starts_with("halted") {
                            "Halted"
                        } else {
                            self.status.as_str()
                        })
                        .color(color),
                    )
                    .on_hover_text(&self.status);
                    if ui.button("Disconnect").clicked() {
                        self.send(Work::Disconnect);
                    }
                    ui.separator();
                    if ui.button("Halt").clicked() {
                        self.send(Work::Control("halt"));
                    }
                    if ui.button("Resume").clicked() {
                        self.send(Work::Control("resume"));
                    }
                    if ui.toggle_value(&mut self.paused, "Pause polling").changed() {
                        self.send(Work::Polling(self.paused));
                    }
                } else {
                    ui.label(RichText::new(&self.status).weak());
                    if ui
                        .add_enabled(
                            !self.connecting,
                            egui::Button::new(if self.connecting {
                                "Connecting…"
                            } else {
                                "Connect"
                            }),
                        )
                        .clicked()
                    {
                        self.notice = None;
                        self.connecting = true;
                        self.send(Work::Connect(self.saved.clone()));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.saved.poll_ms)
                                .range(20..=10000)
                                .suffix(" ms"),
                        )
                        .changed()
                    {
                        self.configure();
                    }
                    ui.label(RichText::new("Poll").weak());
                });
            });
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Connection")
                .default_open(!self.connected)
                .show(ui, |ui| {
                    ui.add_enabled_ui(!self.connected && !self.connecting, |ui| {
                        egui::Grid::new("connection-fields")
                            .num_columns(2)
                            .spacing([12.0, 7.0])
                            .show(ui, |ui| {
                                ui.label("Probe");
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("probe")
                                        .selected_text(if self.saved.probe.is_empty() {
                                            "Any probe"
                                        } else {
                                            &self.saved.probe
                                        })
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.saved.probe,
                                                String::new(),
                                                "Any probe",
                                            );
                                            for (selector, name) in &self.probes {
                                                ui.selectable_value(
                                                    &mut self.saved.probe,
                                                    selector.clone(),
                                                    format!("{selector}  {name}"),
                                                );
                                            }
                                        });
                                    if ui.small_button("Refresh").clicked() {
                                        self.send(Work::List(self.saved.cli.clone()));
                                    }
                                });
                                ui.end_row();
                                ui.label("ELF");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.saved.elf)
                                            .desired_width(600.0),
                                    );
                                    if ui.small_button("Browse…").clicked()
                                        && let Some(path) = rfd::FileDialog::new()
                                            .set_title("Firmware ELF")
                                            .pick_file()
                                    {
                                        self.saved.elf = path.to_string_lossy().into_owned();
                                    }
                                });
                                ui.end_row();
                                ui.label("Chip");
                                ui.text_edit_singleline(&mut self.saved.chip);
                                ui.end_row();
                                ui.label("SWD");
                                ui.add(
                                    egui::DragValue::new(&mut self.saved.speed)
                                        .range(100..=50000)
                                        .suffix(" kHz"),
                                );
                                ui.end_row();
                                ui.label("CLI");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.saved.cli)
                                        .desired_width(600.0),
                                );
                                ui.end_row();
                            });
                    });
                    if self.connected {
                        ui.weak(if self.owned {
                            "Connected through the session this app started."
                        } else {
                            "Using an existing session; disconnect leaves it running."
                        });
                    }
                });
        });
    }

    fn add_watch(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("+").weak());
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.add)
                    .id(egui::Id::new("add-watch"))
                    .hint_text("Add a variable or task path…")
                    .desired_width(ui.available_width() - 65.0),
            );
            if response.changed() {
                self.suggestions.clear();
                self.suggestion_error = None;
                self.selected_suggestion = 0;
                self.suggestion_due = Some(Instant::now() + Duration::from_millis(180));
            }
            if response.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.selected_suggestion = (self.selected_suggestion + 1)
                        .min(self.suggestions.len().saturating_sub(1));
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.selected_suggestion = self.selected_suggestion.saturating_sub(1);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.suggestions.clear();
                    if self.replacing.take().is_some() {
                        self.add.clear();
                    }
                }
            }
            if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui
                    .button(if self.replacing.is_some() {
                        "Replace"
                    } else {
                        "Add"
                    })
                    .clicked()
            {
                if let Some(item) = self.suggestions.get(self.selected_suggestion)
                    && item.path != self.add
                {
                    self.add = item.path.clone();
                    self.suggestions.clear();
                    response.request_focus();
                } else {
                    self.insert_watch();
                    response.request_focus();
                }
            }
        });
        if !self.suggestions.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("suggestions")
                .max_height(170.0)
                .show(ui, |ui| {
                    let mut chosen = None;
                    for (index, item) in self.suggestions.iter().enumerate() {
                        let text = format!("{}    {}", item.path, item.type_name);
                        if ui
                            .selectable_label(index == self.selected_suggestion, tail(&text, 105))
                            .on_hover_text(format!("{}\n{}", item.path, item.label))
                            .clicked()
                        {
                            chosen = Some(item.path.clone());
                        }
                    }
                    if let Some(path) = chosen {
                        self.add = path;
                        self.suggestions.clear();
                        ui.memory_mut(|m| m.request_focus(egui::Id::new("add-watch")));
                    }
                });
        } else if let Some(error) = &self.suggestion_error {
            ui.weak(error);
        }
    }

    fn insert_watch(&mut self) {
        let path = self.add.trim();
        if !path.is_empty() && !self.saved.watches.iter().any(|v| v == path) {
            if self.saved.watches.len() >= 64 && self.replacing.is_none() {
                self.notice = Some("At most 64 added watches".into());
                return;
            }
            if let Some(previous) = self.replacing.take() {
                if let Some(index) = self.saved.watches.iter().position(|p| p == &previous) {
                    self.saved.watches[index] = path.to_owned();
                }
            } else {
                self.saved.watches.push(path.to_owned());
            }
            self.configure();
        }
        self.add.clear();
        self.replacing = None;
        self.suggestions.clear();
        self.suggestion_error = None;
    }

    fn begin_edit(&mut self, value: &Value) {
        if self.saved.locked.contains(&value.path) {
            self.reason = Some(value.clone());
            return;
        }
        if !value.writable || value.error.is_some() {
            self.reason = Some(value.clone());
            return;
        }
        if self.connected {
            self.edit = Some(Edit {
                path: value.path.clone(),
                text: value.raw(),
                focus: true,
                pending: false,
                error: None,
            });
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, value: &Value, depth: usize, root: Option<&str>) {
        let expanded = self.saved.expanded.contains(&value.path);
        let has_children = !value.children.is_empty() || value.expandable;
        let locked = self.saved.locked.contains(&value.path);
        let editing = self.edit.as_ref().is_some_and(|e| e.path == value.path);
        let width = ui.available_width();
        let name_width = (width * 0.43 - depth as f32 * 16.0).max(80.0);
        let mut context_response: Option<egui::Response> = None;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0);
            if has_children {
                if ui
                    .add_sized(
                        [18.0, 22.0],
                        egui::Button::new(if expanded { "−" } else { "+" }).frame(false),
                    )
                    .clicked()
                {
                    if expanded {
                        self.saved.expanded.remove(&value.path);
                        if self
                            .edit
                            .as_ref()
                            .is_some_and(|e| model::descendant(&e.path, &value.path))
                        {
                            self.edit = None;
                        }
                    } else if self.saved.expanded.len() < 64 {
                        self.saved.expanded.insert(value.path.clone());
                    }
                    self.configure();
                }
            } else {
                ui.add_space(26.0);
            }
            let name = value.label.as_deref().unwrap_or(&value.path);
            let max_chars = (name_width / 7.3).max(8.0) as usize;
            let label = ui
                .allocate_ui_with_layout(
                    egui::vec2(name_width, 22.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(tail(name, max_chars)).monospace())
                                .sense(egui::Sense::click()),
                        )
                    },
                )
                .inner;
            context_response = Some(label.clone());
            if label.clicked() {
                self.selected = Some(value.path.clone());
            }
            label.on_hover_text(format!(
                "{}\n{}\n{}",
                value.path,
                value.type_name,
                value.reason()
            ));
            if editing {
                let Some(edit) = self.edit.as_mut() else {
                    return;
                };
                let input = ui.add_enabled(
                    !edit.pending,
                    egui::TextEdit::singleline(&mut edit.text)
                        .id_salt((&value.path, "edit"))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width()),
                );
                let newly_focused = edit.focus;
                if edit.focus {
                    input.request_focus();
                    edit.focus = false;
                }
                if !edit.pending
                    && (input.has_focus() || input.lost_focus())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    edit.pending = true;
                    edit.error = None;
                    let work = Work::Set {
                        path: edit.path.clone(),
                        input: edit.text.clone(),
                    };
                    self.send(work);
                } else if !edit.pending
                    && (ui.input(|i| i.key_pressed(egui::Key::Escape))
                        || (input.lost_focus() && !newly_focused))
                {
                    self.edit = None;
                }
            } else {
                let text = value.error.clone().unwrap_or_else(|| {
                    if value
                        .display
                        .lines()
                        .next()
                        .is_some_and(|line| line.contains(" = "))
                    {
                        value.raw()
                    } else {
                        value.type_name.clone()
                    }
                });
                let color = if value.error.is_some()
                    || !value.writable
                    || locked
                    || self.pending_confirm.contains(&value.path)
                {
                    ui.visuals().weak_text_color()
                } else {
                    Color32::from_rgb(132, 193, 235)
                };
                let button = ui
                    .allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 22.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(tail(
                                        &text,
                                        ((width - name_width) / 7.3).max(12.0) as usize,
                                    ))
                                    .monospace()
                                    .color(color),
                                )
                                .sense(egui::Sense::click()),
                            )
                        },
                    )
                    .inner;
                context_response = Some(
                    context_response
                        .take()
                        .map_or_else(|| button.clone(), |response| response.union(button.clone())),
                );
                if button.clicked() {
                    self.begin_edit(value);
                }
                button.on_hover_text(format!(
                    "{}\n{}",
                    value.display,
                    if locked {
                        "Locked in Live Watch"
                    } else {
                        value.reason()
                    }
                ));
            }
        });
        if let Some(response) = context_response {
            response.context_menu(|ui| {
                if ui
                    .add_enabled(
                        value.writable && !locked && self.connected,
                        egui::Button::new("Edit value"),
                    )
                    .clicked()
                {
                    self.begin_edit(value);
                    ui.close();
                }
                if ui.button(if locked { "Unlock" } else { "Lock" }).clicked() {
                    if locked {
                        self.saved.locked.remove(&value.path);
                    } else {
                        self.saved.locked.insert(value.path.clone());
                    }
                    self.configure();
                    ui.close();
                }
                if ui.button("Copy value").clicked() {
                    ui.ctx().copy_text(value.raw());
                    ui.close();
                }
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(value.path.clone());
                    ui.close();
                }
                if ui.button("Details").clicked() {
                    self.reason = Some(value.clone());
                    ui.close();
                }
                if !value.writable
                    && value.error.is_none()
                    && ui.button("Suggest watch_mut!").clicked()
                {
                    self.reason = Some(value.clone());
                    ui.close();
                }
                if let Some(root) = root
                    && ui.button("Edit watch path").clicked()
                {
                    self.add = root.to_owned();
                    self.replacing = Some(root.to_owned());
                    self.suggestions.clear();
                    self.suggestion_due = Some(Instant::now());
                    ui.memory_mut(|m| m.request_focus(egui::Id::new("add-watch")));
                    ui.close();
                }
                if let Some(root) = root
                    && ui.button("Remove watch").clicked()
                {
                    self.saved.watches.retain(|p| p != root);
                    self.configure();
                    ui.close();
                }
            });
        }
        if let Some(edit) = &self.edit
            && edit.path == value.path
        {
            if edit.pending {
                ui.horizontal(|ui| {
                    ui.add_space(name_width + 30.0);
                    ui.spinner();
                    ui.weak("Waiting for firmware…");
                });
            }
            if let Some(error) = &edit.error {
                ui.colored_label(Color32::from_rgb(224, 143, 143), error);
            }
        }
        if expanded {
            for child in &value.children {
                self.row(ui, child, depth + 1, root);
            }
            if let Some(note) = &value.children_note {
                ui.weak(note);
            }
            if value.children.is_empty() && value.expandable {
                ui.weak("Waiting for fields…");
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events();
        self.top(ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.reason = None;
        }
        if self.suggestion_due.is_some_and(|due| Instant::now() >= due) {
            self.suggestion_due = None;
            self.configure();
            self.send(Work::Complete(self.add.clone()));
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.edit.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::F2) || i.key_pressed(egui::Key::Enter))
            && !ctx.wants_keyboard_input()
            && let Some(path) = &self.selected
        {
            let values = self
                .declared
                .iter()
                .chain(self.watched.iter().flat_map(|(_, v)| v))
                .cloned()
                .collect::<Vec<_>>();
            if let Some(value) = model::find(&values, path) {
                self.begin_edit(value);
            }
        }
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.weak(format!(
                    "{} declared · {} added",
                    self.declared.len(),
                    self.saved.watches.len()
                ));
                if let Some(at) = self.last_sample {
                    ui.weak(format!("Updated {:.1}s ago", at.elapsed().as_secs_f32()));
                }
                if self.paused {
                    ui.weak("Polling paused");
                }
            });
            egui::CollapsingHeader::new("Session log").show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(130.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.monospace(line);
                        }
                    });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Live Watch");
            });
            if let Some(message) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(message).color(Color32::from_rgb(224, 170, 117)));
                    if ui.small_button("×").clicked() {
                        self.notice = None;
                    }
                });
            }
            if let Some(error) = &self.read_error {
                ui.weak(error);
            }
            self.add_watch(ui);
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width((ui.available_width() - 10.0).max(500.0));
                    if self.declared.is_empty() && self.watched.is_empty() {
                        ui.add_space(25.0);
                        ui.weak(if self.connected {
                            "No declared values. Add a symbol or task path above."
                        } else {
                            "Choose the probe and firmware ELF, then connect."
                        });
                    }
                    for value in self.declared.clone() {
                        self.row(ui, &value, 0, None);
                    }
                    if !self.declared.is_empty() && !self.watched.is_empty() {
                        ui.separator();
                    }
                    for (root, values) in self.watched.clone() {
                        for value in values {
                            self.row(ui, &value, 0, Some(&root));
                        }
                    }
                });
        });
        if let Some(value) = self.reason.clone() {
            let mut open = true;
            egui::Window::new("Watch details").id(egui::Id::new("watch-details")).open(&mut open).default_width(460.0).show(ctx, |ui| {
                ui.monospace(&value.path); ui.weak(&value.type_name); ui.separator();
                ui.label(if self.saved.locked.contains(&value.path) { "Locked in Live Watch. Unlock from the context menu to edit." } else { value.reason() });
                if !value.writable && value.error.is_none() {
                    ui.add_space(8.0); ui.label("Expose a mutable scalar where the task uses it:");
                    let scalar = if ["f32", "bool", "u8", "u16", "u32", "i8", "i16", "i32"].contains(&value.type_name.as_str()) { value.type_name.as_str() } else { "f32" };
                    let snippet = format!("tuning_studio_api::watch_mut!({:?}: {} = &mut expression);", format!("{}.edit", value.path), scalar);
                    ui.add(egui::Label::new(RichText::new(&snippet).monospace()).wrap());
                    ui.weak("Replace expression with the local or field. Use one call site per value. The edit applies at the next call.");
                    if ui.button("Copy suggestion").clicked() { ctx.copy_text(snippet); }
                }
            });
            if !open {
                self.reason = None;
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "session", &self.saved);
    }
}

fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    if count <= chars {
        text.to_owned()
    } else {
        format!(
            "…{}",
            text.chars().skip(count - chars + 1).collect::<String>()
        )
    }
}
