use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;

use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Icon, Window, WindowBuilder};

use super::app::{self, EditorConfig};
use super::ui::{self, Fonts, Painter, Rect, Rgba, Theme, Ui, icon};
use super::{PendingInput, RecentProject, load_recent_projects, record_recent_project};

const HUB_W: f64 = 940.0;
const HUB_H: f64 = 620.0;
const HUB_MIN_W: f64 = 560.0;
const HUB_MIN_H: f64 = 420.0;

fn load_hub_icon() -> Option<Icon> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logo.png");
    let image = image::open(path).ok()?.to_rgba8();
    let resized = image::imageops::resize(&image, 64, 64, image::imageops::FilterType::Nearest);
    Icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

fn load_hub_fonts(config: &EditorConfig) -> Result<Fonts, String> {
    let configured_font = config.settings.font_path.trim();
    if configured_font.is_empty() {
        return ui::load_fonts();
    }
    let path = PathBuf::from(configured_font);
    ui::load_fonts_from_path(Some(&path)).or_else(|error| {
        eprintln!("warning: {error}; falling back to bundled editor font");
        ui::load_fonts()
    })
}

pub fn run_hub() -> Result<(), String> {
    let config_path = app::global_config_path();
    let config = app::load_config(&config_path);
    let mut fonts = load_hub_fonts(&config)?;
    let mut hub = HubApp::new(config, config_path);

    let event_loop = EventLoop::new();
    let mut window_builder = WindowBuilder::new()
        .with_title("NeoLOVE Hub")
        .with_inner_size(LogicalSize::new(HUB_W, HUB_H))
        .with_min_inner_size(LogicalSize::new(HUB_MIN_W, HUB_MIN_H));
    if let Some(icon) = load_hub_icon() {
        window_builder = window_builder.with_window_icon(Some(icon));
    }
    let window = window_builder
        .build(&event_loop)
        .map_err(|error| format!("failed to create Hub window: {error}"))?;

    let context = unsafe { softbuffer::Context::new(&window) }
        .map_err(|error| format!("failed to create Hub surface context: {error}"))?;
    let mut surface = unsafe { softbuffer::Surface::new(&context, &window) }
        .map_err(|error| format!("failed to create Hub surface: {error}"))?;

    let mut input = PendingInput::default();
    window.request_redraw();

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => {
                handle_hub_event(event, &window, &mut input, control_flow);
            }
            Event::RedrawRequested(_) => {
                if let Err(error) = redraw_hub(
                    &window,
                    &mut surface,
                    &mut hub,
                    &mut input,
                    &fonts,
                ) {
                    eprintln!("Hub render error: {error}");
                    *control_flow = ControlFlow::Exit;
                    return;
                }

                if hub.take_font_reload_request() {
                    match load_hub_fonts(&hub.config) {
                        Ok(next_fonts) => {
                            fonts = next_fonts;
                            window.request_redraw();
                        }
                        Err(error) => {
                            hub.status = error;
                            window.request_redraw();
                        }
                    }
                }

                if let Some(project) = hub.take_launch_project() {
                    match launch_editor(&project) {
                        Ok(()) => *control_flow = ControlFlow::Exit,
                        Err(error) => {
                            hub.status = error;
                            window.request_redraw();
                        }
                    }
                }

                if hub.should_quit {
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}

struct HubApp {
    config: EditorConfig,
    config_path: PathBuf,
    project_name: String,
    parent_dir: String,
    status: String,
    recents: Vec<RecentProject>,
    settings: Option<SettingsDraft>,
    focus: Option<String>,
    edit_buffer: String,
    edit_cursor: usize,
    edit_selection_anchor: Option<usize>,
    pointer_capture: Option<String>,
    reload_fonts: bool,
    launch_project: Option<PathBuf>,
    should_quit: bool,
}

#[derive(Clone)]
struct SettingsDraft {
    theme_name: String,
    font_path: String,
    show_tooltips: bool,
    show_window_bounds: bool,
    show_transform_hud: bool,
    autosave_before_run: bool,
    autosave_before_build: bool,
}

impl SettingsDraft {
    fn from_config(config: &EditorConfig) -> Self {
        Self {
            theme_name: config.settings.theme_name.clone(),
            font_path: config.settings.font_path.clone(),
            show_tooltips: config.settings.show_tooltips,
            show_window_bounds: config.settings.show_window_bounds,
            show_transform_hud: config.settings.show_transform_hud,
            autosave_before_run: config.settings.autosave_before_run,
            autosave_before_build: config.settings.autosave_before_build,
        }
    }
}

impl HubApp {
    fn new(config: EditorConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            project_name: "my-game".to_string(),
            parent_dir: default_projects_dir().to_string_lossy().to_string(),
            status: String::new(),
            recents: load_recent_projects(),
            settings: None,
            focus: None,
            edit_buffer: String::new(),
            edit_cursor: 0,
            edit_selection_anchor: None,
            pointer_capture: None,
            reload_fonts: false,
            launch_project: None,
            should_quit: false,
        }
    }

    fn frame(&mut self, ui: &mut Ui) {
        let w = ui.painter.width();
        let h = ui.painter.height();
        let compact = w < 760.0;
        let modal_open = self.settings.is_some();
        let bg = ui.theme.viewport_bg;
        let panel = ui.theme.panel;
        let panel_alt = ui.theme.panel_alt;
        let line = ui.theme.border;
        let accent = ui.theme.accent;

        ui.painter.clear(bg);

        if modal_open {
            ui.set_input_clip(Rect::new(-1.0, -1.0, 0.0, 0.0));
        }

        ui.painter.text(24.0, 22.0, "NeoLOVE Hub", 22.0, ui.theme.text);
        ui.painter.text(26.0, 50.0, "Projects", 13.0, ui.theme.text_dim);
        let settings = Rect::new((w - 196.0).max(24.0), 24.0, 172.0, 32.0);
        let settings_clicked = ui.icon_button(settings, icon::TUNE, "Editor Settings");
        if !modal_open && settings_clicked {
            self.open_settings();
        }
        ui.painter.fill_rect(Rect::new(24.0, 70.0, w - 48.0, 1.0), line);

        if compact {
            let top = Rect::new(18.0, 90.0, w - 36.0, 218.0);
            self.draw_new_project(ui, top, panel, panel_alt, accent, !modal_open);
            let lower = Rect::new(18.0, top.bottom() + 18.0, w - 36.0, h - top.bottom() - 36.0);
            self.draw_open_and_recent(ui, lower, panel, panel_alt, !modal_open);
        } else {
            let left = Rect::new(24.0, 94.0, 330.0, h - 118.0);
            let right = Rect::new(
                left.right() + 24.0,
                94.0,
                w - left.right() - 48.0,
                h - 118.0,
            );
            self.draw_new_project(ui, left, panel, panel_alt, accent, !modal_open);
            self.draw_open_and_recent(ui, right, panel, panel_alt, !modal_open);
        }

        if modal_open {
            ui.reset_input_clip();
        }

        if !self.status.is_empty() {
            let status_h = 28.0;
            let status = Rect::new(18.0, h - status_h - 12.0, w - 36.0, status_h);
            ui.painter.fill_round_rect(status, 4.0, [0, 0, 0, 185]);
            ui.painter.text_clipped(
                status.x + 10.0,
                status.y + 6.0,
                &self.status,
                13.0,
                ui.theme.text,
                status.w - 20.0,
            );
        }

        if modal_open {
            self.draw_settings(ui, w, h);
        }
    }

    fn draw_new_project(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        panel: Rgba,
        panel_alt: Rgba,
        warm: Rgba,
        interactive: bool,
    ) {
        ui.painter.fill_round_rect(area, 6.0, panel);
        ui.painter.stroke_round_rect(area, 6.0, ui.theme.border);
        ui.painter
            .icon_centered(area.x + 25.0, area.y + 29.0, icon::ADD_CIRCLE, 20.0, warm);
        ui.painter.text(
            area.x + 44.0,
            area.y + 18.0,
            "New Project",
            17.0,
            ui.theme.text,
        );

        let field_w = (area.w - 28.0).max(80.0);
        let mut y = area.y + 58.0;
        ui.painter
            .text(area.x + 14.0, y - 18.0, "Name", 12.0, ui.theme.text_dim);
        let name = ui.text_field(
            "hub_project_name",
            Rect::new(area.x + 14.0, y, field_w, 30.0),
            &self.project_name,
        );
        if name.changed {
            self.project_name = name.text;
        }

        y += 58.0;
        ui.painter
            .text(area.x + 14.0, y - 18.0, "Location", 12.0, ui.theme.text_dim);
        let choose_w = 42.0;
        let location_w = (field_w - choose_w - 8.0).max(80.0);
        let location = ui.text_field(
            "hub_parent_dir",
            Rect::new(area.x + 14.0, y, location_w, 30.0),
            &self.parent_dir,
        );
        if location.changed {
            self.parent_dir = location.text;
        }
        let choose = Rect::new(area.x + 14.0 + location_w + 8.0, y, choose_w, 30.0);
        let choose_clicked = ui.icon_button(choose, icon::FOLDER_OPEN, "");
        if interactive && choose_clicked {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose Project Location")
                .pick_folder()
            {
                self.parent_dir = path.to_string_lossy().to_string();
            }
        }

        y += 52.0;
        let preview_path = PathBuf::from(self.parent_dir.trim()).join(self.project_name.trim());
        ui.painter
            .fill_round_rect(Rect::new(area.x + 14.0, y, field_w, 24.0), 3.0, panel_alt);
        ui.painter.text_clipped(
            area.x + 22.0,
            y + 5.0,
            &preview_path.to_string_lossy(),
            12.0,
            ui.theme.text_dim,
            field_w - 16.0,
        );

        let create = Rect::new(area.x + 14.0, area.bottom() - 44.0, field_w, 32.0);
        let create_clicked = ui.icon_button(create, icon::ADD, "Create Project");
        if interactive && (create_clicked || ui.input.enter) {
            self.create_project();
        }
    }

    fn draw_open_and_recent(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        panel: Rgba,
        panel_alt: Rgba,
        interactive: bool,
    ) {
        ui.painter.fill_round_rect(area, 6.0, panel);
        ui.painter.stroke_round_rect(area, 6.0, ui.theme.border);

        let load = Rect::new(
            area.x + 14.0,
            area.y + 14.0,
            178.0_f32.min(area.w - 28.0),
            34.0,
        );
        let load_clicked = ui.icon_button(load, icon::FOLDER_OPEN, "Load Folder");
        if interactive && load_clicked {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Open NeoLOVE Project")
                .pick_folder()
            {
                self.open_project(path);
            }
        }

        ui.painter.icon_centered(
            area.x + 24.0,
            area.y + 78.0,
            icon::HISTORY,
            18.0,
            ui.theme.text_dim,
        );
        ui.painter
            .text(area.x + 42.0, area.y + 67.0, "Recents", 17.0, ui.theme.text);

        let list_top = area.y + 100.0;
        let row_h = 54.0;
        let max_rows = ((area.bottom() - list_top - 14.0) / row_h).floor().max(0.0) as usize;
        if self.recents.is_empty() {
            let empty = Rect::new(area.x + 14.0, list_top, area.w - 28.0, 62.0);
            ui.painter.fill_round_rect(empty, 4.0, panel_alt);
            ui.painter.text_clipped(
                empty.x + 12.0,
                empty.y + 21.0,
                "No recent projects yet.",
                14.0,
                ui.theme.text_dim,
                (empty.w - 24.0).max(0.0),
            );
            return;
        }

        let visible_count = self.recents.len().min(max_rows);
        for idx in 0..visible_count {
            let row = Rect::new(
                area.x + 14.0,
                list_top + idx as f32 * row_h,
                area.w - 28.0,
                row_h - 8.0,
            );
            let project = self.recents[idx].clone();
            if draw_recent_row(ui, row, &project, panel_alt) && interactive {
                self.open_project(project.path);
            }
        }
    }

    fn create_project(&mut self) {
        let name = self.project_name.trim();
        if name.is_empty() {
            self.status = "Project name is required.".to_string();
            return;
        }
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            self.status = "Project name cannot contain path separators.".to_string();
            return;
        }
        let parent = self.parent_dir.trim();
        if parent.is_empty() {
            self.status = "Project location is required.".to_string();
            return;
        }

        let project_path = PathBuf::from(parent).join(name);
        match crate::create_project_at(&project_path, name) {
            Ok(path) => self.open_project(path),
            Err(error) => self.status = error,
        }
    }

    fn open_project(&mut self, path: PathBuf) {
        match crate::validate_project_root(&path) {
            Ok(()) => {
                self.status.clear();
                self.launch_project = Some(path);
            }
            Err(error) => {
                self.status = error;
                self.recents = load_recent_projects();
            }
        }
    }

    fn take_launch_project(&mut self) -> Option<PathBuf> {
        self.launch_project.take()
    }

    fn take_font_reload_request(&mut self) -> bool {
        std::mem::take(&mut self.reload_fonts)
    }

    fn open_settings(&mut self) {
        let draft = SettingsDraft::from_config(&self.config);
        self.focus = Some("hub_editor_font_path".to_string());
        self.edit_buffer = draft.font_path.clone();
        self.edit_cursor = self.edit_buffer.chars().count();
        self.edit_selection_anchor = None;
        self.settings = Some(draft);
    }

    fn draw_settings(&mut self, ui: &mut Ui, w: f32, h: f32) {
        let Some(mut draft) = self.settings.take() else {
            return;
        };

        ui.painter.fill_rect(Rect::new(0.0, 0.0, w, h), [0, 0, 0, 135]);
        let width = (w - 32.0).min(560.0).max(360.0);
        let height = (h - 24.0).min(460.0).max(340.0);
        let compact = height < 430.0;
        let theme_row_h = if compact { 18.0 } else { 20.0 };
        let theme_area_h = theme_row_h * app::theme_presets().len() as f32 + 8.0;
        let toggle_step = if compact { 23.0 } else { 25.0 };
        let px = (w - width) * 0.5;
        let py = (h - height) * 0.5;
        let panel = Rect::new(px, py, width, height);
        ui.painter.fill_round_rect(panel, 6.0, ui.theme.panel);
        ui.painter.stroke_round_rect(panel, 6.0, ui.theme.accent);
        ui.painter
            .text(px + 16.0, py + 14.0, "Editor Settings", 17.0, ui.theme.text);

        let mut y = py + if compact { 40.0 } else { 46.0 };
        ui.painter.text(px + 16.0, y, "Theme", 14.0, ui.theme.text_dim);
        y += if compact { 16.0 } else { 18.0 };
        let theme_area = Rect::new(px + 16.0, y, width - 32.0, theme_area_h);
        ui.painter.fill_rect(theme_area, ui.theme.field);
        ui.painter.stroke_rect(theme_area, ui.theme.border);
        let mut row_y = theme_area.y + 4.0;
        for (name, label) in app::theme_presets() {
            let row = Rect::new(theme_area.x + 4.0, row_y, theme_area.w - 8.0, theme_row_h);
            if ui.list_row(row, label, draft.theme_name == *name, 0.0) {
                draft.theme_name = (*name).to_string();
            }
            row_y += theme_row_h;
        }

        y = theme_area.bottom() + if compact { 10.0 } else { 12.0 };
        ui.painter.text(px + 16.0, y + 4.0, "Font Path", 13.0, ui.theme.text_dim);
        let font_result = ui.text_field(
            "hub_editor_font_path",
            Rect::new(px + 106.0, y, width - 122.0, 22.0),
            &draft.font_path,
        );
        if font_result.changed {
            draft.font_path = font_result.text;
        }
        y += if compact { 28.0 } else { 32.0 };

        for (label, value) in [
            ("Show tooltips", &mut draft.show_tooltips),
            ("Show default window bounds", &mut draft.show_window_bounds),
            ("Show Scene transform HUD", &mut draft.show_transform_hud),
            ("Autosave before Run", &mut draft.autosave_before_run),
            ("Autosave before Build", &mut draft.autosave_before_build),
        ] {
            ui.painter.text_clipped(
                px + 16.0,
                y + 4.0,
                label,
                13.0,
                ui.theme.text_dim,
                214.0_f32.min(width - 48.0).max(0.0),
            );
            if let Some(next) = ui.checkbox(Rect::new(px + 240.0, y, 22.0, 22.0), *value) {
                *value = next;
            }
            y += toggle_step;
        }

        let save = Rect::new(panel.right() - 204.0, panel.bottom() - 36.0, 92.0, 26.0);
        let cancel = Rect::new(panel.right() - 104.0, panel.bottom() - 36.0, 90.0, 26.0);
        if ui.button_colored(save, "Save", ui.theme.button, ui.theme.text) {
            self.save_settings(draft);
            return;
        }
        if ui.button(cancel, "Cancel") || ui.input.escape {
            self.focus = None;
            self.edit_buffer.clear();
            self.edit_cursor = 0;
            self.edit_selection_anchor = None;
            return;
        }

        self.settings = Some(draft);
    }

    fn save_settings(&mut self, draft: SettingsDraft) {
        self.config.settings.theme_name = draft.theme_name.clone();
        self.config.settings.font_path = draft.font_path.trim().to_string();
        self.config.settings.show_tooltips = draft.show_tooltips;
        self.config.settings.show_window_bounds = draft.show_window_bounds;
        self.config.settings.show_transform_hud = draft.show_transform_hud;
        self.config.settings.autosave_before_run = draft.autosave_before_run;
        self.config.settings.autosave_before_build = draft.autosave_before_build;
        if let Some(theme) = app::theme_preset(&draft.theme_name) {
            self.config.theme = theme;
        }

        match app::save_config(&self.config_path, &self.config) {
            Ok(()) => {
                self.status = format!("Saved editor settings ({})", app::theme_label(&draft.theme_name));
                self.settings = None;
                self.focus = None;
                self.edit_buffer.clear();
                self.edit_cursor = 0;
                self.edit_selection_anchor = None;
                self.reload_fonts = true;
            }
            Err(error) => {
                self.status = error;
                self.settings = Some(draft);
            }
        }
    }
}

fn draw_recent_row(ui: &mut Ui, row: Rect, project: &RecentProject, bg: Rgba) -> bool {
    let hovered = row.contains(ui.input.mouse_x, ui.input.mouse_y);
    let fill = if hovered { ui.theme.button_active } else { bg };
    ui.painter.fill_round_rect(row, 4.0, fill);
    if hovered {
        ui.painter.stroke_round_rect(row, 4.0, ui.theme.accent);
    }
    ui.painter.icon_centered(
        row.x + 20.0,
        row.y + row.h * 0.5,
        icon::FOLDER,
        18.0,
        ui.theme.text_dim,
    );
    ui.painter.text_clipped(
        row.x + 40.0,
        row.y + 8.0,
        &project.name,
        14.0,
        ui.theme.text,
        row.w - 52.0,
    );
    ui.painter.text_clipped(
        row.x + 40.0,
        row.y + 27.0,
        &project.path.to_string_lossy(),
        11.0,
        ui.theme.text_dim,
        row.w - 52.0,
    );
    hovered && ui.input.mouse_pressed
}

fn default_projects_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(home) = home {
        let documents = home.join("Documents");
        if documents.is_dir() {
            return documents.join("NeoLOVE Projects");
        }
        return home.join("NeoLOVE Projects");
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn handle_hub_event(
    event: WindowEvent,
    window: &Window,
    input: &mut PendingInput,
    control_flow: &mut ControlFlow,
) {
    match event {
        WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
        WindowEvent::ModifiersChanged(state) => {
            input.ctrl = state.ctrl() || state.logo();
            input.shift = state.shift();
        }
        WindowEvent::CursorMoved { position, .. } => {
            input.mouse_x = position.x as f32;
            input.mouse_y = position.y as f32;
            window.request_redraw();
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let pressed = state == ElementState::Pressed;
            if button == MouseButton::Left {
                input.mouse_down = pressed;
                if pressed {
                    input.mouse_pressed = true;
                }
                window.request_redraw();
            }
        }
        WindowEvent::MouseWheel { delta, .. } => {
            input.scroll += match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(pos) => (pos.y as f32) / 32.0,
            };
            window.request_redraw();
        }
        WindowEvent::ReceivedCharacter(ch) => {
            if !ch.is_control() {
                input.typed.push(ch);
                window.request_redraw();
            }
        }
        WindowEvent::KeyboardInput {
            input:
                KeyboardInput {
                    virtual_keycode: Some(key),
                    state: ElementState::Pressed,
                    ..
                },
            ..
        } => {
            match key {
                VirtualKeyCode::Back => input.backspace = true,
                VirtualKeyCode::Delete => input.delete = true,
                VirtualKeyCode::Return | VirtualKeyCode::NumpadEnter => input.enter = true,
                VirtualKeyCode::Escape => input.escape = true,
                VirtualKeyCode::C if input.ctrl => input.copy = true,
                VirtualKeyCode::V if input.ctrl => input.paste = true,
                VirtualKeyCode::X if input.ctrl => input.cut = true,
                VirtualKeyCode::A if input.ctrl => input.select_all = true,
                VirtualKeyCode::Left => input.left = true,
                VirtualKeyCode::Right => input.right = true,
                VirtualKeyCode::Home => input.home = true,
                VirtualKeyCode::End => input.end = true,
                _ => {}
            }
            window.request_redraw();
        }
        WindowEvent::Resized(_) => window.request_redraw(),
        _ => {}
    }
}

fn redraw_hub(
    window: &Window,
    surface: &mut softbuffer::Surface,
    hub: &mut HubApp,
    input: &mut PendingInput,
    fonts: &Fonts,
) -> Result<(), String> {
    let size = window.inner_size();
    let width = size.width.max(1);
    let height = size.height.max(1);

    surface
        .resize(
            NonZeroU32::new(width).expect("width clamped to >= 1"),
            NonZeroU32::new(height).expect("height clamped to >= 1"),
        )
        .map_err(|error| format!("failed to resize Hub surface: {error}"))?;

    let mut buffer = surface
        .buffer_mut()
        .map_err(|error| format!("failed to acquire Hub buffer: {error}"))?;

    let painter = Painter::new(&mut buffer, width as usize, height as usize, fonts.clone());
    let frame_input = input.take_frame();
    let theme = hub_theme(&hub.config.theme);
    let mut ctx = Ui::new(
        painter,
        frame_input,
        theme,
        hub.focus.take(),
        std::mem::take(&mut hub.edit_buffer),
        std::mem::take(&mut hub.edit_cursor),
        hub.edit_selection_anchor.take(),
        hub.pointer_capture.take(),
    );
    hub.frame(&mut ctx);
    let wants_redraw = ctx.wants_redraw;
    let (focus, edit_buffer, edit_cursor, edit_selection_anchor, pointer_capture) =
        ctx.into_focus_state();
    hub.focus = focus;
    hub.edit_buffer = edit_buffer;
    hub.edit_cursor = edit_cursor;
    hub.edit_selection_anchor = edit_selection_anchor;
    hub.pointer_capture = pointer_capture;

    buffer
        .present()
        .map_err(|error| format!("failed to present Hub surface: {error}"))?;

    if wants_redraw {
        window.request_redraw();
    }
    Ok(())
}

fn hub_theme(editor_theme: &Theme) -> Theme {
    editor_theme.clone()
}

fn launch_editor(path: &Path) -> Result<(), String> {
    if let Err(error) = record_recent_project(path) {
        eprintln!("warning: failed to update recent projects: {error}");
    }

    let exe = std::env::current_exe()
        .map_err(|error| format!("failed to resolve NeoLOVE executable: {error}"))?;
    let mut command = Command::new(exe);
    command.arg("editor").arg(path);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map_err(|error| format!("failed to launch editor: {error}"))?;
    Ok(())
}
