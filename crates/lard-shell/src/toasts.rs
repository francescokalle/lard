//! Notification toasts. The D-Bus daemon lives in `lard-services`; this module
//! owns timing, stacking, animation and dismissal.

use crate::widgets::truncate;
use lard_core::anim::{Spring, SpringConfig};
use lard_core::theme::NotificationConfig;
use lard_core::{Point, Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use lard_services::notifications::{Notification, NotificationAction};
use std::time::{Duration, Instant};

/// How long a notification takes over the bar's centre island.
const PREVIEW_TTL: Duration = Duration::from_millis(6000);
/// Decay of the preview at the end of that window: the island shrinks and
/// fades back to the clock instead of disappearing abruptly.
const PREVIEW_DECAY: Duration = Duration::from_millis(1400);

struct Toast {
    notification: Notification,
    app: String,
    summary: String,
    body: String,
    actions: Vec<NotificationAction>,
    height: f32,
    progress: Spring,
    deadline: Option<Instant>,
    /// When the toast arrived: it only takes over the centre island for a
    /// short while, even if the notification itself never expires.
    preview_at: Instant,
    closing: bool,
}

/// One notification of the notification center, with its actions.
#[derive(Clone, Debug)]
pub struct NotificationEntry {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    /// (key, label) of the D-Bus actions.
    pub actions: Vec<(String, String)>,
}

/// Compact preview of the newest notification, rendered in place of the
/// clock inside the bar's centre island.
#[derive(Clone, Debug)]
pub struct ToastHeadline {
    pub app: String,
    pub summary: String,
    pub body: String,
    pub progress: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastPress {
    Dismiss(u32),
    Action { id: u32, action: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ToastHit {
    Dismiss(u32),
    Action { id: u32, action: String },
}

pub struct ToastStack {
    toasts: Vec<Toast>,
    reduce_motion: bool,
    spring: SpringConfig,
    hits: Vec<(Rect, ToastHit)>,
    cursor: Option<Point>,
}

impl ToastStack {
    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            toasts: Vec::with_capacity(4),
            reduce_motion,
            spring: animation,
            hits: Vec::with_capacity(4),
            cursor: None,
        }
    }

    /// Push a toast. Returns the id of a toast that had to be evicted, if any.
    pub fn push(&mut self, notification: Notification, config: &NotificationConfig) -> Option<u32> {
        if let Some(existing) = self
            .toasts
            .iter_mut()
            .find(|t| t.notification.id == notification.id)
        {
            existing.app = display_app(&notification);
            existing.summary = truncate(&notification.summary, 120);
            existing.body = truncate(&notification.body.replace('\n', " "), 400);
            existing.actions = notification.actions.clone();
            existing.height = toast_height(existing.actions.len());
            existing.notification = notification;
            existing.closing = false;
            existing.progress.set_target(1.0);
            existing.deadline = deadline(existing.notification.timeout_ms);
            existing.preview_at = Instant::now();
            return None;
        }

        let mut evicted = None;
        if self.toasts.len() >= config.max_visible {
            let oldest = self.toasts.pop();
            evicted = oldest.map(|t| t.notification.id);
        }
        let deadline = deadline(notification.timeout_ms);
        self.toasts.insert(
            0,
            Toast {
                app: display_app(&notification),
                summary: truncate(&notification.summary, 120),
                body: truncate(&notification.body.replace('\n', " "), 400),
                actions: notification.actions.clone(),
                height: toast_height(notification.actions.len()),
                notification,
                progress: Spring::new(if self.reduce_motion { 1.0 } else { 0.0 }, self.spring),
                deadline,
                preview_at: Instant::now(),
                closing: false,
            },
        );
        if !self.reduce_motion {
            self.toasts[0].progress.set_target(1.0);
        }
        evicted
    }

    pub fn remove(&mut self, id: u32) {
        self.toasts.retain(|t| t.notification.id != id);
    }

    pub fn dismiss(&mut self, id: u32) {
        if let Some(toast) = self.toasts.iter_mut().find(|t| t.notification.id == id) {
            toast.closing = true;
            toast.deadline = None;
            if self.reduce_motion {
                toast.progress.snap_to(0.0);
            } else {
                toast.progress.set_target(0.0);
            }
        }
    }

    /// Advance animations and expiry. Returns ids that expired.
    /// Restart the entrance of the visible toasts once the surface is mapped,
    /// so the island grows in instead of already being at full width.
    pub fn start_entrance(&mut self) {
        if self.reduce_motion {
            return;
        }
        for toast in &mut self.toasts {
            if !toast.closing {
                toast.progress.snap_to(0.0);
                toast.progress.set_target(1.0);
            }
        }
    }

    pub fn update(&mut self, dt: f32) -> Vec<u32> {
        let mut expired = Vec::new();
        for toast in &mut self.toasts {
            toast.progress.update(dt);
            if let Some(deadline) = toast.deadline
                && Instant::now() >= deadline
                && !toast.closing
            {
                toast.closing = true;
                toast.deadline = None;
                expired.push(toast.notification.id);
                if self.reduce_motion {
                    toast.progress.snap_to(0.0);
                } else {
                    toast.progress.set_target(0.0);
                }
            }
        }
        self.toasts
            .retain(|t| !(t.closing && t.progress.value() <= 0.01));
        expired
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.toasts.len()
    }

    /// The notification currently shown in the expanded centre island: the
    /// newest one that is still visible, with its decay applied.
    #[must_use]
    pub fn preview(&self) -> Option<ToastHeadline> {
        let now = Instant::now();
        let toast = self
            .toasts
            .iter()
            .find(|toast| !toast.closing && toast.progress.value() > 0.01)?;
        let elapsed = now.saturating_duration_since(toast.preview_at);
        let fade = decay_factor(elapsed);
        if fade <= 0.0 {
            return None;
        }
        Some(ToastHeadline {
            app: toast.app.clone(),
            summary: if toast.summary.is_empty() {
                toast.body.clone()
            } else {
                toast.summary.clone()
            },
            body: toast.body.clone(),
            progress: toast.progress.value().clamp(0.0, 1.0) * fade,
        })
    }

    /// The notifications waiting in the notification center.
    #[must_use]
    pub fn entries(&self) -> Vec<NotificationEntry> {
        self.toasts
            .iter()
            .filter(|toast| !toast.closing)
            .map(|toast| NotificationEntry {
                id: toast.notification.id,
                app: toast.app.clone(),
                summary: toast.summary.clone(),
                body: toast.body.clone(),
                actions: toast
                    .actions
                    .iter()
                    .map(|action| (action.key.clone(), action.label.clone()))
                    .collect(),
            })
            .collect()
    }

    /// Notifications waiting in the notification center.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.toasts.iter().filter(|toast| !toast.closing).count()
    }

    pub fn active(&self) -> bool {
        self.toasts.iter().any(|t| !t.progress.is_settled())
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.toasts.iter().filter_map(|t| t.deadline).min()
    }

    #[must_use]
    pub fn preferred_size(&self, config: &NotificationConfig) -> Option<Size> {
        if self.toasts.is_empty() {
            return None;
        }
        let height = 12.0
            + self.toasts.iter().map(|toast| toast.height).sum::<f32>()
            + (self.toasts.len().saturating_sub(1)) as f32 * config.gap;
        Some(Size::new(config.width, height))
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.cursor = Some(pos);
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    /// Returns the action to perform for a click on a toast.
    pub fn pointer_press(&mut self, pos: Point) -> Option<ToastPress> {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, hit)| hit.clone())?;
        match hit {
            ToastHit::Dismiss(id) => {
                self.dismiss(id);
                Some(ToastPress::Dismiss(id))
            }
            ToastHit::Action { id, action } => {
                self.dismiss(id);
                Some(ToastPress::Action { id, action })
            }
        }
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        config: &NotificationConfig,
        theme: &lard_core::theme::Theme,
        fonts: &lard_core::theme::FontConfig,
    ) {
        self.hits.clear();
        let mut y = 6.0;
        for toast in &self.toasts {
            let p = toast.progress.value().clamp(0.0, 1.0);
            let alpha = p;
            let slide = (1.0 - p) * 26.0;
            let rect = Rect::new(slide, y, size.w - 4.0, toast.height);
            let bg = theme.surface.with_alpha(0.96 * alpha);
            scene.bordered(
                rect,
                bg,
                theme.radius.card(),
                theme.border_width,
                theme.border.scale_alpha(alpha),
            );
            scene.rect(
                Rect::new(rect.x + 10.0, rect.y + 12.0, 3.0, rect.h - 24.0),
                theme.accent.scale_alpha(alpha),
                1.5,
            );

            scene.text(
                toast.app.as_str(),
                Point::new(rect.x + 24.0, rect.y + 12.0),
                TextStyle {
                    size: fonts.size_small - 0.5,
                    weight: fonts.weight_medium,
                    ..Default::default()
                },
                theme.accent.scale_alpha(0.85 * alpha),
            );
            scene.text(
                toast.summary.as_str(),
                Point::new(rect.x + 24.0, rect.y + 28.0),
                TextStyle {
                    size: fonts.size_body - 0.5,
                    weight: fonts.weight_semibold,
                    ..Default::default()
                },
                theme.text.scale_alpha(alpha),
            );
            if !toast.body.is_empty() {
                scene.text(
                    crate::widgets::truncate_to_width(
                        toast.body.as_str(),
                        fonts.size_small - 0.5,
                        rect.w - 48.0,
                    ),
                    Point::new(rect.x + 24.0, rect.y + 47.0),
                    TextStyle {
                        size: fonts.size_small - 0.5,
                        max_width: Some(rect.w - 48.0),
                        wrap: false,
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(0.82 * alpha),
                );
            }
            let hot = self.cursor.is_some_and(|c| rect.contains(c));
            if hot {
                scene.text(
                    "×",
                    Point::new(rect.right() - 28.0, rect.y + 10.0),
                    TextStyle {
                        size: fonts.size_body,
                        align: TextAlign::Right,
                        max_width: Some(14.0),
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(alpha),
                );
            }
            self.hits
                .push((rect, ToastHit::Dismiss(toast.notification.id)));
            if !toast.actions.is_empty() {
                let mut x = rect.x + 24.0;
                for action in toast.actions.iter().take(2) {
                    let width = (action.label.chars().count() as f32 * 6.2 + 24.0).min(136.0);
                    let button = Rect::new(x, rect.y + 72.0, width, 24.0);
                    let hover = self.cursor.is_some_and(|c| button.contains(c));
                    scene.rect(
                        button,
                        theme
                            .accent
                            .with_alpha(if hover { 0.24 } else { 0.14 } * alpha),
                        8.0,
                    );
                    scene.text_center_y(
                        truncate(&action.label, 18),
                        button.x,
                        button.center().y,
                        TextStyle {
                            size: fonts.size_small - 0.5,
                            weight: fonts.weight_medium,
                            align: TextAlign::Center,
                            max_width: Some(button.w),
                            ..Default::default()
                        },
                        theme.text.scale_alpha(0.92 * alpha),
                    );
                    self.hits.push((
                        button,
                        ToastHit::Action {
                            id: toast.notification.id,
                            action: action.key.clone(),
                        },
                    ));
                    x += width + 6.0;
                }
            }
            y += toast.height + config.gap;
        }
    }
}

/// Opacity of the preview over time: full while the notification is fresh,
/// then a linear decay back to zero.
fn decay_factor(elapsed: Duration) -> f32 {
    if elapsed >= PREVIEW_TTL {
        return 0.0;
    }
    let remaining = PREVIEW_TTL.saturating_sub(elapsed);
    if remaining < PREVIEW_DECAY {
        remaining.as_secs_f32() / PREVIEW_DECAY.as_secs_f32()
    } else {
        1.0
    }
}

fn deadline(timeout_ms: u32) -> Option<Instant> {
    (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(u64::from(timeout_ms)))
}

fn display_app(notification: &Notification) -> String {
    if notification.app_name.is_empty() {
        "Notifica".to_string()
    } else {
        truncate(&notification.app_name, 36)
    }
}

fn toast_height(actions: usize) -> f32 {
    if actions == 0 { 76.0 } else { 104.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_decays_to_zero() {
        assert_eq!(decay_factor(Duration::from_millis(0)), 1.0);
        assert_eq!(decay_factor(Duration::from_millis(3000)), 1.0);
        let half = decay_factor(PREVIEW_TTL - PREVIEW_DECAY / 2);
        assert!(half > 0.4 && half < 0.6, "{half}");
        assert_eq!(decay_factor(PREVIEW_TTL), 0.0);
        assert_eq!(decay_factor(PREVIEW_TTL + Duration::from_secs(5)), 0.0);
    }
}
