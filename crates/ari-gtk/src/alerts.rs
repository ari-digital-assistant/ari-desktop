//! A sounding alert: its sound, played over until it's stopped or its caps
//! run out. The notification that goes with it is posted by Ari.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use ari_core::envelope::AlertSpec;
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk4::glib;
use tracing::warn;

/// The quiet between one play of an alert's sound and the next, as on the
/// phone: 150 ms after the sound, where its spoken words go, then 900 ms.
const BETWEEN_RINGS: Duration = Duration::from_millis(1050);

pub struct Sounding {
    pub spec: AlertSpec,
    player: Option<Player>,
    /// Ends the alert at `auto_stop_ms`, until it has.
    auto_stop: Rc<Cell<Option<glib::SourceId>>>,
}

impl Sounding {
    /// Start sounding `spec` with the sound in `sound`, if any. `over` is
    /// called once, when the alert has gone on as long as `spec` allows.
    pub fn start(spec: AlertSpec, sound: Option<&Path>, over: impl Fn() + 'static) -> Self {
        let over = Rc::new(over);
        let auto_stop = Rc::new(Cell::new(None));
        let player = sound.and_then(|sound| play(sound, spec.max_cycles.max(1), over.clone()));
        let ms = u64::try_from(spec.auto_stop_ms).unwrap_or(0);
        if ms > 0 {
            let fired = auto_stop.clone();
            let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
                // It has run, so there's nothing left to remove.
                fired.set(None);
                over();
            });
            auto_stop.set(Some(id));
        }
        Self {
            spec,
            player,
            auto_stop,
        }
    }

    pub fn stop(self) {
        if let Some(player) = &self.player
            && let Err(error) = player.playbin.set_state(gst::State::Null)
        {
            warn!(%error, "the alert's sound wouldn't stop");
        }
        if let Some(id) = self.auto_stop.take() {
            id.remove();
        }
    }
}

/// A sound playing, and the watch on its messages, which ends with it.
struct Player {
    playbin: gst::Element,
    _watch: gst::bus::BusWatchGuard,
}

/// Play `sound` `cycles` times, then call `over`. None when it can't be.
fn play(sound: &Path, cycles: i64, over: Rc<dyn Fn()>) -> Option<Player> {
    let started = || -> Result<Player, glib::BoolError> {
        gst::init().map_err(|error| glib::bool_error!("{error}"))?;
        let uri =
            glib::filename_to_uri(sound, None).map_err(|error| glib::bool_error!("{error}"))?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .build()?;
        // Sound only: a video track never opens a window.
        playbin.set_property_from_str("flags", "audio");
        let bus = playbin
            .bus()
            .ok_or_else(|| glib::bool_error!("playbin has no bus"))?;
        let weak = playbin.downgrade();
        let played = Cell::new(1);
        let path = sound.to_path_buf();
        let watch = bus.add_watch_local(move |_, message| {
            match message.view() {
                gst::MessageView::Eos(_) if played.get() < cycles => {
                    played.set(played.get() + 1);
                    // A stopped alert's player has gone by the time this runs.
                    let weak = weak.clone();
                    glib::timeout_add_local_once(BETWEEN_RINGS, move || {
                        if let Some(playbin) = weak.upgrade()
                            && let Err(error) =
                                playbin.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO)
                        {
                            warn!(%error, "the alert's sound wouldn't play again");
                        }
                    });
                }
                gst::MessageView::Eos(_) => over(),
                gst::MessageView::Error(error) => {
                    warn!(error = %error.error(), sound = %path.display(), "the alert's sound won't play");
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })?;
        playbin
            .set_state(gst::State::Playing)
            .map_err(|error| glib::bool_error!("{error}"))?;
        Ok(Player {
            playbin,
            _watch: watch,
        })
    };
    started()
        .inspect_err(
            |error| warn!(%error, sound = %sound.display(), "the alert's sound won't play"),
        )
        .ok()
}

/// The file an alert's `sound` names: one of the skill's assets, found by
/// `asset`, or the sound theme's for the system sounds. None for silence, or
/// when the file isn't there.
pub fn sound_file(sound: &str, asset: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    match sound {
        "system.silent" => None,
        "system.alarm" => theme_sound("alarm-clock-elapsed"),
        _ if sound.starts_with("asset:") => {
            asset(sound).or_else(|| theme_sound("message-new-instant"))
        }
        // `system.notification`, and any token this doesn't know.
        _ => theme_sound("message-new-instant"),
    }
}

/// Sound `name` from the freedesktop sound theme, wherever the data
/// directories have it.
fn theme_sound(name: &str) -> Option<PathBuf> {
    let dirs = std::iter::once(glib::user_data_dir()).chain(glib::system_data_dirs());
    let found = dirs
        .flat_map(|dir| {
            ["oga", "ogg", "wav"].map(|ext| {
                dir.join("sounds/freedesktop/stereo")
                    .join(format!("{name}.{ext}"))
            })
        })
        .find(|path| path.is_file());
    if found.is_none() {
        warn!(name, "the sound theme has no such sound");
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_has_no_sound() {
        assert_eq!(sound_file("system.silent", |_| None), None);
    }

    #[test]
    fn a_skill_s_own_sound_is_its_asset() {
        let asset = |reference: &str| {
            (reference == "asset:timer.mp3")
                .then(|| PathBuf::from("/skills/timer/assets/timer.mp3"))
        };
        assert_eq!(
            sound_file("asset:timer.mp3", asset),
            Some(PathBuf::from("/skills/timer/assets/timer.mp3"))
        );
    }
}
