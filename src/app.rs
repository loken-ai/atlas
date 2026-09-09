//! The window.
//!
//! Lives here rather than in the binary so the documentation screenshot can drive the real
//! application: a shot built from a copy of the layout would photograph the copy.
//!
//! Polling runs elsewhere and hands this a finished snapshot. Drawing never waits on a node -
//! a machine that has gone stops answering, and a window that blocked on it would freeze for
//! the timeout of every request, on every frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::doctor::Finding;
use crate::icons::Icon;
use crate::model::ClusterSnapshot;
use crate::view;

/// What the polling task publishes and the window reads. Held behind one lock, so a frame
/// cannot draw a snapshot from one round with the findings of another.
#[derive(Default)]
pub struct Shared {
    pub snapshot: ClusterSnapshot,
    pub findings: Vec<Finding>,
    pub polled_at: Option<Instant>,
    /// Set once at startup when the discovery socket could not be opened.
    pub note: Option<String>,
}

pub struct Gui {
    pub shared: Arc<Mutex<Shared>>,
    pub endpoints: Vec<String>,
    pub every: Duration,
    /// Set by the Refresh button and cleared by the polling task.
    pub refresh_now: Arc<AtomicBool>,
    /// Whether the nodes should measure where a decode step's time goes. The poll thread
    /// carries a change to every node; the window switches it off when it closes.
    pub measure: Arc<AtomicBool>,
}

/// A reading that has aged is not the same as a reading. The window says how old it is rather
/// than presenting the last round as the present.
pub fn age(polled_at: Option<Instant>) -> String {
    match polled_at {
        None => "polling...".to_string(),
        Some(t) => {
            let s = t.elapsed().as_secs();
            if s == 0 {
                "just now".to_string()
            } else {
                format!("{s}s ago")
            }
        }
    }
}

impl eframe::App for Gui {
    /// What the window switched on, it switches off: a node left measuring after the
    /// observer has gone pays the synchronisation for nobody.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.measure.swap(false, Ordering::Relaxed) {
            let endpoints = self.endpoints.clone();
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(crate::collect::set_measuring(&endpoints, false));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (snapshot, findings, polled_at, note) = {
            let shared = self.shared.lock().expect("shared state");
            (
                shared.snapshot.clone(),
                shared.findings.clone(),
                shared.polled_at,
                shared.note.clone(),
            )
        };

        egui::Panel::top("bar")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0x16, 0x1b, 0x22))
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    Icon::Globe.show(ui, 16.0, view::TEXT);
                    ui.label(
                        egui::RichText::new("atlas")
                            .size(15.0)
                            .strong()
                            .color(view::TEXT),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{} nodes polled", self.endpoints.len()))
                            .size(11.0)
                            .color(view::MUTED),
                    );
                    ui.label(
                        egui::RichText::new(format!("every {}s", self.every.as_secs()))
                            .size(11.0)
                            .color(view::MUTED),
                    );
                    ui.label(
                        egui::RichText::new(age(polled_at))
                            .size(11.0)
                            .color(view::MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                Icon::Refresh.image(12.0, view::TEXT),
                                egui::RichText::new("Refresh").size(11.0),
                            ))
                            .clicked()
                        {
                            self.refresh_now.store(true, Ordering::Relaxed);
                        }
                        let mut measure = self.measure.load(Ordering::Relaxed);
                        if ui
                            .checkbox(&mut measure, egui::RichText::new("Layer time").size(11.0))
                            .on_hover_text(
                                "Ask every node where a decode step's time goes, layer by \
                                 layer. Costs the nodes a device synchronisation per stage \
                                 while it is on.",
                            )
                            .changed()
                        {
                            self.measure.store(measure, Ordering::Relaxed);
                            self.refresh_now.store(true, Ordering::Relaxed);
                        }
                    });
                });
            });

        egui::Panel::bottom("doctor")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0x16, 0x1b, 0x22))
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                if let Some(note) = &note {
                    ui.label(
                        egui::RichText::new(note)
                            .size(10.0)
                            .color(view::MUTED)
                            .italics(),
                    );
                }
                view::findings(ui, &findings);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    view::cluster(ui, &snapshot);
                });
        });

        // The age in the top bar is a clock, so the window repaints without an event. Once a
        // second is enough to keep it honest and costs nothing between polls.
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn window(shared: Shared) -> egui_kittest::Harness<'static, Gui> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .build_eframe(move |_cc| Gui {
                shared: Arc::new(Mutex::new(shared)),
                endpoints: vec!["http://192.0.2.10:11435".to_string()],
                every: Duration::from_secs(5),
                refresh_now: Arc::new(AtomicBool::new(false)),
                measure: Arc::new(AtomicBool::new(false)),
            });
        harness.run_steps(3);
        harness
    }

    /// The window is more than the cluster drawing. It was exactly that once - a scroll area
    /// over `view::cluster` and nothing else, with a snapshot that was never filled - so this
    /// asserts the two things that made it an application: the bar that says how fresh the
    /// reading is, and the panel that says what doctor found.
    #[test]
    fn the_window_carries_its_bar_and_its_verdict() {
        let snapshot = ClusterSnapshot {
            cluster: Some("home".into()),
            ..Default::default()
        };
        let findings = vec![crate::doctor::Finding {
            rule: "no-peer-view",
            detail: "http://192.0.2.10:11435 publishes no peer view".into(),
        }];
        let harness = window(Shared {
            snapshot,
            findings,
            polled_at: Some(Instant::now()),
            note: None,
        });
        assert!(harness.query_by_label_contains("Refresh").is_some(), "bar");
        assert!(
            harness.query_by_label_contains("nodes polled").is_some(),
            "how many nodes it is watching"
        );
        assert!(
            harness
                .query_by_label_contains("does not report who it sees")
                .is_some(),
            "doctor's verdict, as a sentence"
        );
    }

    /// Before the first round the bar says so, rather than showing an empty cluster as though
    /// it had been measured.
    #[test]
    fn a_window_that_has_not_polled_yet_says_so() {
        assert_eq!(age(None), "polling...");
        let harness = window(Shared::default());
        assert!(harness.query_by_label_contains("polling").is_some());
        // Nothing measured, so doctor has nothing to say - and says that, rather than
        // leaving a blank panel that reads as broken.
        assert!(harness
            .query_by_label_contains("Nothing to report")
            .is_some());
    }
}
