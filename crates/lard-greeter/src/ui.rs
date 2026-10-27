//! Greeter / lockscreen view.
//!
//! Pure state + draw list: no Wayland or GPU object is touched here. The same
//! view renders into either an `ext-session-lock-v1` surface (lock) or a
//! fullscreen layer surface (greetd greeter).

use crate::auth::{AuthEvent, AuthMode, AuthOutcome};
use crate::sessions::{SessionEntry, User, current_user, discover_sessions, discover_users};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::theme::Theme;
use lard_core::theme::{FontConfig, GreeterConfig};
use lard_core::{Color, Point, Rect, Size};
use lard_render::{Scene, Shadow, TextAlign, TextStyle};
use lard_services::power::PowerAction;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Clone, Debug)]
pub struct SubmitRequest {
    pub username: String,
    pub password: Zeroizing<String>,
    pub session: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub enum GreeterAction {
    Submit(SubmitRequest),
    Power(PowerAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GreeterKey {
    Char(String),
    Backspace,
    Enter,
    Escape,
    Tab { shift: bool },
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GreeterHit {
    SelectUser(usize),
    SelectSession(usize),
    FocusPassword,
    Submit,
    Power(PowerAction),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GreeterHitArea {
    pub rect: Rect,
    pub action: GreeterHit,
}

pub struct GreeterView {
    pub mode: AuthMode,
    pub users: Vec<User>,
    pub selected_user: usize,
    pub sessions: Vec<SessionEntry>,
    pub selected_session: usize,
    pub pending: bool,
    pub outcome: Option<AuthOutcome>,
    success_at: Option<Instant>,
    password: Zeroizing<String>,
    error: Option<String>,
    config: GreeterConfig,
    card: Spring,
    fade: Spring,
    shake: Spring,
    caret_time: f32,
    elapsed: f32,
    cooldown_until: Option<Instant>,
    failed_attempts: u32,
    reduce_motion: bool,
    cursor: Option<Point>,
    clock: String,
    date: String,
    /// Wallpaper shown behind the card: id in the image atlas + RGBA thumbnail.
    wallpaper: Option<(u32, Vec<u8>)>,
}

impl GreeterView {
    /// Decode the desktop wallpaper for the lock screen backdrop. Done once, at
    /// start-up: the lock surface must not block on a 4K JPEG later.
    pub fn load_wallpaper(&mut self, path: &std::path::Path) {
        const ID: u32 = 0x4C4F_434B;
        // The image atlas cell is 128x128: anything larger would be cut and
        // its rows misread (visible as horizontal stripes).
        const W: u32 = 128;
        const H: u32 = 72;
        let Ok(image) = image::open(path) else {
            self.wallpaper = None;
            return;
        };
        let small = image.resize_exact(W, H, image::imageops::FilterType::Triangle);
        self.wallpaper = Some((ID, small.to_rgba8().into_raw()));
    }

    pub fn new(mode: AuthMode, config: &GreeterConfig, reduce_motion: bool) -> Self {
        let users = match mode {
            AuthMode::Lock => vec![current_user()],
            AuthMode::Greeter | AuthMode::Preview => discover_users(),
        };
        let (sessions, selected_session) = discover_sessions(&config.default_session);
        let selected_session = selected_session.min(2);
        let selected_user = users
            .iter()
            .take(6)
            .position(|u| u.name == current_user().name)
            .unwrap_or(0);

        let config_spring = SpringConfig::GENTLE;
        let mut card = Spring::new(0.0, config_spring);
        let mut fade = Spring::new(1.0, config_spring);
        if reduce_motion {
            card.snap_to(1.0);
            fade.snap_to(1.0);
        } else {
            card.set_target(1.0);
        }

        Self {
            mode,
            users,
            selected_user,
            sessions,
            selected_session,
            pending: false,
            outcome: None,
            success_at: None,
            password: Zeroizing::new(String::new()),
            error: None,
            config: config.clone(),
            card,
            fade,
            shake: Spring::new(0.0, SpringConfig::STIFF),
            caret_time: 0.0,
            elapsed: 0.0,
            cooldown_until: None,
            failed_attempts: 0,
            reduce_motion,
            cursor: None,
            clock: String::new(),
            date: String::new(),
            wallpaper: None,
        }
    }

    #[must_use]
    pub fn user(&self) -> Option<&User> {
        self.users.get(self.selected_user)
    }

    #[must_use]
    pub fn session(&self) -> Option<&SessionEntry> {
        self.sessions.get(self.selected_session)
    }

    #[must_use]
    pub fn password_len(&self) -> usize {
        self.password.chars().count()
    }

    pub fn set_clock(&mut self, clock: String, date: String) {
        self.clock = clock;
        self.date = date;
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
            && (self.fade.value() < 0.02
                // Safety net: never let greetd wait on a fade that cannot run
                // (for example because the output stopped presenting frames).
                || self
                    .success_at
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(3)))
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.fade.value() > 0.005
    }

    /// Next instant at which the caret blink changes state.
    ///
    /// The app loop is event driven, so the caret needs a deadline of its own:
    /// without it the greeter would only repaint on the next clock tick.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        if let Some(at) = self.success_at {
            return Some(at + Duration::from_secs(3));
        }
        if self.pending {
            return None;
        }
        const BLINK: f32 = 0.53;
        let phase = self.elapsed % (BLINK * 2.0);
        let remaining = if phase < BLINK {
            BLINK - phase
        } else {
            BLINK * 2.0 - phase
        };
        Some(Instant::now() + Duration::from_secs_f32(remaining.max(0.01)))
    }

    pub fn update(&mut self, dt: f32) -> bool {
        self.elapsed += dt;
        self.caret_time += dt;
        let mut active = self.card.update(dt);
        active |= self.fade.update(dt);
        active |= self.shake.update(dt);
        if let Some(until) = self.cooldown_until
            && Instant::now() >= until
        {
            self.cooldown_until = None;
            active = true;
        }
        active
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.cursor = Some(pos);
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    pub fn pointer_press(&mut self, pos: Point, hits: &[GreeterHitArea]) -> Option<GreeterAction> {
        let hit = hits
            .iter()
            .rev()
            .find(|area| area.rect.contains(pos))
            .map(|area| area.action)?;
        match hit {
            GreeterHit::SelectUser(index) => {
                if index < self.users.len() {
                    self.selected_user = index;
                    self.error = None;
                }
                None
            }
            GreeterHit::SelectSession(index) => {
                if index < self.sessions.len() {
                    self.selected_session = index;
                }
                None
            }
            GreeterHit::FocusPassword => None,
            GreeterHit::Submit => self.submit().map(GreeterAction::Submit),
            GreeterHit::Power(action) => Some(GreeterAction::Power(action)),
        }
    }

    pub fn key(&mut self, key: GreeterKey) -> Option<GreeterAction> {
        if self.outcome.is_some() {
            return None;
        }
        match key {
            GreeterKey::Char(text) => {
                if self.pending || self.cooldown_until.is_some() {
                    return None;
                }
                if self.password.chars().count() < 128 {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.password.push(ch);
                    }
                    self.error = None;
                    self.caret_time = 0.0;
                }
                None
            }
            GreeterKey::Backspace => {
                if !self.pending {
                    self.password.pop();
                    self.caret_time = 0.0;
                }
                None
            }
            GreeterKey::Enter => self.submit().map(GreeterAction::Submit),
            GreeterKey::Escape => {
                self.password = Zeroizing::new(String::new());
                self.error = None;
                None
            }
            GreeterKey::Tab { shift } => {
                self.cycle_user(if shift {
                    Direction::Previous
                } else {
                    Direction::Next
                });
                None
            }
            GreeterKey::Up => {
                self.cycle_user(Direction::Previous);
                None
            }
            GreeterKey::Down => {
                self.cycle_user(Direction::Next);
                None
            }
            GreeterKey::Left => {
                self.cycle_session(Direction::Previous);
                None
            }
            GreeterKey::Right => {
                self.cycle_session(Direction::Next);
                None
            }
        }
    }

    fn cycle_user(&mut self, direction: Direction) {
        if self.users.is_empty() || self.mode == AuthMode::Lock {
            return;
        }
        let len = self.users.len().min(6).max(1);
        self.selected_user = self.selected_user.min(len - 1);
        self.selected_user = match direction {
            Direction::Next => (self.selected_user + 1) % len,
            Direction::Previous => (self.selected_user + len - 1) % len,
        };
        self.error = None;
        self.password = Zeroizing::new(String::new());
    }

    fn cycle_session(&mut self, direction: Direction) {
        if self.sessions.is_empty() || self.mode == AuthMode::Lock {
            return;
        }
        let len = self.sessions.len().min(3).max(1);
        self.selected_session = self.selected_session.min(len - 1);
        self.selected_session = match direction {
            Direction::Next => (self.selected_session + 1) % len,
            Direction::Previous => (self.selected_session + len - 1) % len,
        };
    }

    fn submit(&mut self) -> Option<SubmitRequest> {
        if self.pending || self.outcome.is_some() {
            return None;
        }
        if let Some(until) = self.cooldown_until {
            let remaining = until.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                self.error = Some(format!("Riprova tra {}s", remaining.as_secs() + 1));
                self.bump_shake();
                return None;
            }
            self.cooldown_until = None;
        }

        let user = self.users.get(self.selected_user)?.name.clone();
        if self.password.is_empty() && !self.config.allow_empty_password {
            self.error = Some("Inserisci la password".into());
            self.bump_shake();
            return None;
        }

        let password = std::mem::replace(&mut self.password, Zeroizing::new(String::new()));
        self.pending = true;
        self.error = None;
        self.caret_time = 0.0;
        let session = if self.mode == AuthMode::Lock {
            None
        } else {
            self.session().map(|s| s.exec.clone())
        };
        Some(SubmitRequest {
            username: user,
            password,
            session,
        })
    }

    pub fn on_auth_event(&mut self, event: AuthEvent) {
        self.pending = false;
        match event {
            AuthEvent::Success(outcome) => {
                self.outcome = Some(outcome);
                self.success_at = Some(Instant::now());
                self.error = None;
                self.failed_attempts = 0;
                if self.reduce_motion {
                    self.fade.snap_to(0.0);
                } else {
                    self.fade.set_target(0.0);
                }
            }
            AuthEvent::Failure { message } => {
                self.error = Some(message.chars().take(48).collect());
                self.failed_attempts = self.failed_attempts.saturating_add(1);
                if self.failed_attempts >= self.config.attempts_before_backoff {
                    let exponent = self
                        .failed_attempts
                        .saturating_sub(self.config.attempts_before_backoff)
                        .min(8);
                    let seconds = (1u64 << exponent).min(self.config.max_backoff_secs.max(1));
                    self.cooldown_until = Some(Instant::now() + Duration::from_secs(seconds));
                }
                self.bump_shake();
            }
        }
    }

    fn bump_shake(&mut self) {
        if self.reduce_motion {
            return;
        }
        self.shake.snap_to(11.0);
        self.shake.set_target(0.0);
    }

    #[must_use]
    pub fn cooldown_remaining(&self) -> Option<u64> {
        self.cooldown_until
            .map(|until| until.saturating_duration_since(Instant::now()).as_secs() + 1)
    }

    pub fn build_scene(
        &mut self,
        scene: &mut Scene,
        hits: &mut Vec<GreeterHitArea>,
        size: Size,
        theme: &Theme,
        fonts: &FontConfig,
    ) {
        hits.clear();
        let fade = self.fade.value().clamp(0.0, 1.0);
        if fade <= 0.001 {
            return;
        }

        let wallpaper = &self.config.wallpaper;
        let lock = self.mode == AuthMode::Lock;
        // A session-lock surface must stay opaque until unlock; only the card
        // fades. Exposing the desktop before `unlock()` would leak content.
        let background_alpha = if lock { 1.0 } else { fade };
        let mut drew_wallpaper = false;
        if wallpaper.background == lard_core::theme::LockBackground::Wallpaper
            && let Some((id, rgba)) = &self.wallpaper
        {
            scene.upload_image(*id, 128, 72, rgba.clone());
            scene.image(Rect::new(0.0, 0.0, size.w, size.h), *id, 0.0);
            scene.rect(
                Rect::new(0.0, 0.0, size.w, size.h),
                theme
                    .background
                    .with_alpha(1.0)
                    .scale_alpha((wallpaper.dim * background_alpha).clamp(0.0, 1.0)),
                0.0,
            );
            drew_wallpaper = true;
        }
        if !drew_wallpaper {
            scene.vertical_gradient(
                Rect::new(0.0, 0.0, size.w, size.h),
                wallpaper.top.with_alpha(background_alpha),
                wallpaper.bottom.with_alpha(background_alpha),
                48,
            );
        }
        scene.glow(
            Rect::new(-size.w * 0.1, size.h * 0.35, size.w * 0.7, size.h * 0.8),
            wallpaper.accent_glow.with_alpha(0.22 * background_alpha),
            150.0,
        );
        scene.glow(
            Rect::new(size.w * 0.65, -size.h * 0.25, size.w * 0.55, size.h * 0.6),
            theme.accent.with_alpha(0.10 * background_alpha),
            140.0,
        );

        let card_w = if lock { 392.0 } else { 444.0 };
        let pad = 28.0;
        let avatar_size = if lock { 72.0 } else { 46.0 };
        let name_size = fonts.size_title + 2.0;
        let has_error = self.error.is_some();

        let content_h = pad
            + avatar_size
            + 14.0
            + name_size
            + 4.0
            + fonts.size_small
            + 20.0
            + 46.0
            + 8.0
            + if has_error { 20.0 } else { 0.0 }
            + if lock { 14.0 } else { 52.0 }
            + pad;
        let card_h = content_h;

        let p = self.card.value().clamp(0.0, 1.0);
        let base_y = (size.h - card_h) * 0.5 + (1.0 - p) * 22.0 + if lock { 34.0 } else { 0.0 };
        let shake = self.shake.value();
        let card = Rect::new(
            (size.w - card_w) * 0.5 + shake,
            base_y + shake * 0.35,
            card_w,
            card_h,
        );
        scene.shadow(
            card,
            24.0,
            Shadow {
                color: theme.shadow.scale_alpha(0.85 * fade),
                blur: (theme.shadow_blur * 1.5).clamp(8.0, 72.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 28.0),
                spread: 2.0,
            },
        );
        scene.bordered(
            card,
            theme.surface.with_alpha(0.90 * fade),
            24.0,
            theme.border_width,
            theme.border.scale_alpha(fade),
        );

        // Brand + clock.
        scene.text(
            "LARD",
            Point::new(32.0, 26.0),
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.9 * fade),
        );
        let clock_style = TextStyle {
            size: if lock {
                (fonts.size_large * 1.25).max(24.0)
            } else {
                22.0
            },
            weight: fonts.weight_semibold,
            align: TextAlign::Right,
            max_width: Some(260.0),
            ..Default::default()
        };
        scene.text(
            &self.clock,
            Point::new(size.w - 32.0 - 260.0, if lock { 42.0 } else { 22.0 }),
            clock_style,
            theme.text.scale_alpha(fade),
        );
        scene.text(
            &self.date,
            Point::new(size.w - 32.0 - 260.0, if lock { 88.0 } else { 48.0 }),
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Right,
                max_width: Some(260.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(fade),
        );

        let mut y = card.y + pad;

        if lock {
            if let Some(user) = self.user() {
                let avatar = Rect::centered(
                    Point::new(card.center().x, y + avatar_size * 0.5),
                    Size::splat(avatar_size),
                );
                self.draw_avatar(scene, avatar, user, theme, fonts, fade, false);
                y += avatar_size + 14.0;
                let display_name: String = user.display_name.chars().take(28).collect();
                scene.text(
                    &display_name,
                    Point::new(card.x + pad, y),
                    TextStyle {
                        size: name_size,
                        weight: fonts.weight_semibold,
                        align: TextAlign::Center,
                        max_width: Some(card.w - pad * 2.0),
                        ..Default::default()
                    },
                    theme.text.scale_alpha(fade),
                );
                y += name_size + 4.0;
                scene.text(
                    "Sessione bloccata",
                    Point::new(card.x + pad, y),
                    TextStyle {
                        size: fonts.size_small,
                        align: TextAlign::Center,
                        max_width: Some(card.w - pad * 2.0),
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(fade),
                );
                y += fonts.size_small + 20.0;
            }
        } else {
            // User picker: avatars in a centered row.
            let visible = self.users.len().min(6);
            let spacing = 14.0;
            let row_w = visible as f32 * avatar_size + (visible.saturating_sub(1)) as f32 * spacing;
            let mut x = card.center().x - row_w * 0.5;
            for index in 0..visible {
                let user = &self.users[index];
                let avatar = Rect::new(x, y, avatar_size, avatar_size);
                let selected = index == self.selected_user;
                self.draw_avatar(scene, avatar, user, theme, fonts, fade, selected);
                hits.push(GreeterHitArea {
                    rect: avatar.inset(-6.0),
                    action: GreeterHit::SelectUser(index),
                });
                x += avatar_size + spacing;
            }
            y += avatar_size + 14.0;
            if let Some(user) = self.user() {
                let display_name: String = user.display_name.chars().take(28).collect();
                scene.text(
                    &display_name,
                    Point::new(card.x + pad, y),
                    TextStyle {
                        size: name_size,
                        weight: fonts.weight_semibold,
                        align: TextAlign::Center,
                        max_width: Some(card.w - pad * 2.0),
                        ..Default::default()
                    },
                    theme.text.scale_alpha(fade),
                );
                y += name_size + 4.0;
                let handle = format!("@{}", user.name);
                let handle: String = handle.chars().take(28).collect();
                scene.text(
                    &handle,
                    Point::new(card.x + pad, y),
                    TextStyle {
                        size: fonts.size_small,
                        align: TextAlign::Center,
                        max_width: Some(card.w - pad * 2.0),
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(fade),
                );
                y += fonts.size_small + 20.0;
            }
        }

        // Password field.
        let field = Rect::new(card.x + pad, y, card.w - pad * 2.0, 46.0);
        let focused = self.outcome.is_none();
        let border_color = if self.error.is_some() {
            theme.danger.scale_alpha(fade)
        } else if focused {
            theme.accent.scale_alpha(0.75 * fade)
        } else {
            theme.border.scale_alpha(fade)
        };
        scene.bordered(
            field,
            theme.background.with_alpha(0.62 * fade),
            12.0,
            1.5,
            border_color,
        );
        hits.push(GreeterHitArea {
            rect: field,
            action: GreeterHit::FocusPassword,
        });

        let dots = self.password.chars().count().min(20);
        if dots == 0 {
            scene.text(
                "Password",
                Point::new(field.x + 16.0, field.y + 14.0),
                TextStyle {
                    size: fonts.size_body,
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * fade),
            );
        } else {
            let spacing = 12.0;
            let width = (dots.saturating_sub(1)) as f32 * spacing;
            let start = field.center().x - width * 0.5 - 2.5;
            for i in 0..dots {
                scene.rect(
                    Rect::new(start + i as f32 * spacing, field.center().y - 2.5, 5.0, 5.0),
                    theme.text.scale_alpha(fade),
                    2.5,
                );
            }
        }
        if focused && !self.pending && (self.elapsed % 1.06) < 0.53 {
            let caret_x = if dots == 0 {
                field.x + 16.0
            } else {
                field.center().x + (dots.saturating_sub(1) as f32 * 12.0) * 0.5 + 12.0
            };
            scene.rect(
                Rect::new(caret_x, field.center().y - 9.0, 1.5, 18.0),
                theme.accent.scale_alpha(fade),
                0.75,
            );
        }
        let submit = Rect::new(field.right() - 38.0, field.y + 8.0, 30.0, 30.0);
        scene.rect(
            submit,
            theme
                .accent
                .with_alpha(if self.password.is_empty() { 0.07 } else { 0.16 } * fade),
            9.0,
        );
        scene.text_center_y(
            "↵",
            submit.x,
            submit.center().y,
            TextStyle {
                size: fonts.size_body,
                align: TextAlign::Center,
                max_width: Some(submit.w),
                ..Default::default()
            },
            theme.accent.scale_alpha(0.85 * fade),
        );
        hits.push(GreeterHitArea {
            rect: submit,
            action: GreeterHit::Submit,
        });
        y = field.bottom() + 8.0;

        if let Some(error) = &self.error {
            scene.text(
                error,
                Point::new(card.x + pad, y + 2.0),
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_medium,
                    align: TextAlign::Center,
                    max_width: Some(card.w - pad * 2.0),
                    ..Default::default()
                },
                theme.danger.scale_alpha(fade),
            );
            y += 20.0;
        }

        if !lock {
            // Session picker.
            y += 6.0;
            let count = self.sessions.len().min(3);
            let mut widths = Vec::with_capacity(count);
            for session in self.sessions.iter().take(count) {
                widths.push(approx_width(&session.name, fonts.size_small) + 26.0);
            }
            let total: f32 = widths.iter().sum::<f32>() + (count.saturating_sub(1)) as f32 * 6.0;
            let mut x = card.center().x - total * 0.5;
            for index in 0..count {
                let w = widths[index];
                let pill = Rect::new(x, y, w, 28.0);
                let selected = index == self.selected_session;
                if selected {
                    scene.rect(pill, theme.accent.scale_alpha(fade), 9.0);
                } else {
                    scene.rect(pill, theme.surface_hover.scale_alpha(0.55 * fade), 9.0);
                }
                scene.text_center_y(
                    &self.sessions[index].name,
                    pill.x + 13.0,
                    pill.center().y,
                    TextStyle {
                        size: fonts.size_small,
                        weight: fonts.weight_medium,
                        align: TextAlign::Left,
                        max_width: Some(w - 26.0),
                        ..Default::default()
                    },
                    if selected {
                        theme.accent_text.scale_alpha(fade)
                    } else {
                        theme.text_muted.scale_alpha(fade)
                    },
                );
                hits.push(GreeterHitArea {
                    rect: pill,
                    action: GreeterHit::SelectSession(index),
                });
                x += w + 6.0;
            }
        }

        // Status line below the card.
        let status = if self.pending {
            Some("Verifica in corso…".to_string())
        } else {
            self.cooldown_remaining()
                .map(|s| format!("Riprova tra {s}s"))
        };
        if let Some(status) = status {
            scene.text(
                &status,
                Point::new(card.x, card.bottom() + 16.0),
                TextStyle {
                    size: fonts.size_small,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(fade),
            );
        }

        // Power row (greeter only).
        if !lock && self.config.show_power_buttons {
            let actions = if self.config.show_suspend {
                vec![
                    PowerAction::Suspend,
                    PowerAction::Reboot,
                    PowerAction::PowerOff,
                ]
            } else {
                vec![PowerAction::Reboot, PowerAction::PowerOff]
            };
            let mut widths = Vec::with_capacity(actions.len());
            for action in &actions {
                widths.push(approx_width(action.label(), fonts.size_small) + 30.0);
            }
            let total: f32 =
                widths.iter().sum::<f32>() + (actions.len().saturating_sub(1)) as f32 * 8.0;
            let mut x = size.w * 0.5 - total * 0.5;
            let y = size.h - 54.0;
            for (index, action) in actions.iter().enumerate() {
                let w = widths[index];
                let button = Rect::new(x, y, w, 30.0);
                let hot = self.cursor.is_some_and(|p| button.contains(p));
                let bg = if *action == PowerAction::PowerOff {
                    theme
                        .danger
                        .with_alpha(if hot { 0.35 } else { 0.18 } * fade)
                } else {
                    theme.surface.with_alpha(if hot { 0.9 } else { 0.6 } * fade)
                };
                scene.rect(button, bg, 10.0);
                scene.text_center_y(
                    action.label(),
                    button.x + 15.0,
                    button.center().y,
                    TextStyle {
                        size: fonts.size_small,
                        weight: fonts.weight_medium,
                        ..Default::default()
                    },
                    theme.text.scale_alpha(0.9 * fade),
                );
                hits.push(GreeterHitArea {
                    rect: button,
                    action: GreeterHit::Power(*action),
                });
                x += w + 8.0;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_avatar(
        &self,
        scene: &mut Scene,
        rect: Rect,
        user: &User,
        theme: &Theme,
        fonts: &FontConfig,
        fade: f32,
        selected: bool,
    ) {
        let tint = avatar_tint(&user.name, theme);
        scene.rect(
            rect.inset(-3.0),
            if selected {
                theme.accent.scale_alpha(0.35 * fade)
            } else {
                Color::TRANSPARENT
            },
            rect.w * 0.5 + 3.0,
        );
        scene.rect(rect, tint.scale_alpha(fade), rect.w * 0.5);
        scene.text_center_y(
            user.initials(),
            rect.x,
            rect.center().y,
            TextStyle {
                size: rect.h * 0.38,
                weight: fonts.weight_semibold,
                align: TextAlign::Center,
                max_width: Some(rect.w),
                ..Default::default()
            },
            theme.text.scale_alpha(fade),
        );
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Next,
    Previous,
}

fn avatar_tint(name: &str, theme: &Theme) -> Color {
    let mut hash = 0u32;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    match hash % 4 {
        0 => theme.accent.with_alpha(0.32),
        1 => theme.success.with_alpha(0.28),
        2 => theme.warning.with_alpha(0.28),
        _ => theme.danger.with_alpha(0.24),
    }
}

fn approx_width(text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| {
            if c.is_whitespace() {
                0.32
            } else if c.is_ascii_uppercase() {
                0.66
            } else {
                0.55
            }
        })
        .sum::<f32>()
        * size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn greeter(reduce_motion: bool) -> GreeterView {
        GreeterView::new(AuthMode::Greeter, &GreeterConfig::default(), reduce_motion)
    }

    #[test]
    fn typing_updates_the_password_mask() {
        let mut view = greeter(true);
        assert_eq!(view.password_len(), 0);
        view.key(GreeterKey::Char("a".into()));
        view.key(GreeterKey::Char("b".into()));
        assert_eq!(view.password_len(), 2);
        view.key(GreeterKey::Backspace);
        assert_eq!(view.password_len(), 1);
        view.key(GreeterKey::Escape);
        assert_eq!(view.password_len(), 0);
    }

    #[test]
    fn caret_deadline_is_armed_while_idle() {
        let view = greeter(false);
        let deadline = view.next_deadline().expect("caret deadline while idle");
        let wait = deadline.saturating_duration_since(Instant::now());
        assert!(wait <= Duration::from_millis(1060), "{wait:?}");
    }

    #[test]
    fn success_finishes_without_fading_frames() {
        let mut view = greeter(true);
        view.on_auth_event(AuthEvent::Success(AuthOutcome::SessionStarted));
        assert!(view.is_finished(), "reduce motion snaps the fade out");
        assert!(
            view.next_deadline().is_some(),
            "the finish watchdog must stay armed for greetd"
        );
    }
}
