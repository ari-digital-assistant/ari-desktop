//! Media controls over MPRIS: the player that's playing, or else one that's
//! paused, does what a skill's `media` action asks.

use ari_core::envelope::MediaAction;
use glib::prelude::*;
use gtk4::{gio, glib};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// How long a player gets to answer, in ms.
const TIMEOUT: i32 = 2000;

/// What a media action asks of a player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// An MPRIS Player method: `Pause`, `Play`, `Next`, `Previous`, `Stop`.
    Method(&'static str),
    /// Set the volume to this (0 to 1).
    Volume(f64),
    /// Change the volume by this.
    Louder(f64),
    Mute,
    Unmute,
}

/// What came of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(Command),
    /// The volume it's at now, as a percentage.
    VolumeAt(u32),
    NothingPlaying,
    Failed(String),
}

/// The command `action` is, or None when it isn't one this can carry out.
pub fn command(action: &MediaAction) -> Option<Command> {
    Some(match action.action.as_str() {
        "pause" => Command::Method("Pause"),
        "resume" => Command::Method("Play"),
        "next" => Command::Method("Next"),
        "previous" => Command::Method("Previous"),
        "stop" => Command::Method("Stop"),
        "volume" => match (action.mute, action.level, action.direction.as_deref()) {
            (Some(true), _, _) => Command::Mute,
            (Some(false), _, _) => Command::Unmute,
            (None, Some(level), _) => Command::Volume(level.clamp(0, 100) as f64 / 100.0),
            (None, None, Some("up")) => Command::Louder(0.1),
            (None, None, Some("down")) => Command::Louder(-0.1),
            _ => return None,
        },
        _ => return None,
    })
}

/// The player to act on, from each player's name and playback status: one
/// playing, else one paused, else none.
pub fn pick(players: &[(String, String)]) -> Option<&str> {
    ["Playing", "Paused"].iter().find_map(|wanted| {
        players
            .iter()
            .find(|(_, status)| status == wanted)
            .map(|(name, _)| name.as_str())
    })
}

/// Carry out `command`. `muted` is the volume a mute left behind, for the
/// unmute after it.
pub async fn control(command: Command, muted: &std::cell::Cell<Option<f64>>) -> Outcome {
    match run(command, muted).await {
        Ok(outcome) => outcome,
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

async fn run(
    command: Command,
    muted: &std::cell::Cell<Option<f64>>,
) -> Result<Outcome, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let names = call(
        &bus,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListNames",
        None,
    )
    .await?;
    let names: Vec<String> = names.child_value(0).get().unwrap_or_default();
    let mut players = Vec::new();
    for name in names.into_iter().filter(|n| n.starts_with(PREFIX)) {
        // A player that doesn't answer is left out rather than failing the rest.
        if let Ok(status) = property(&bus, &name, "PlaybackStatus").await
            && let Some(status) = status.get::<String>()
        {
            players.push((name, status));
        }
    }
    let Some(player) = pick(&players).map(str::to_string) else {
        return Ok(Outcome::NothingPlaying);
    };
    let volume = match command {
        Command::Method(method) => {
            call(&bus, &player, PATH, PLAYER, method, None).await?;
            return Ok(Outcome::Done(command));
        }
        Command::Volume(volume) => volume,
        Command::Louder(by) => {
            let now = property(&bus, &player, "Volume").await?;
            (now.get::<f64>().unwrap_or(1.0) + by).clamp(0.0, 1.0)
        }
        Command::Mute => {
            let now = property(&bus, &player, "Volume").await?;
            muted.set(now.get::<f64>());
            0.0
        }
        Command::Unmute => muted.take().unwrap_or(1.0),
    };
    let set = (PLAYER, "Volume", volume.to_variant()).to_variant();
    call(&bus, &player, PATH, PROPERTIES, "Set", Some(&set)).await?;
    Ok(match command {
        Command::Mute | Command::Unmute => Outcome::Done(command),
        _ => Outcome::VolumeAt((volume * 100.0).round() as u32),
    })
}

async fn call(
    bus: &gio::DBusConnection,
    name: &str,
    path: &str,
    interface: &str,
    method: &str,
    parameters: Option<&glib::Variant>,
) -> Result<glib::Variant, glib::Error> {
    bus.call_future(
        Some(name),
        path,
        interface,
        method,
        parameters,
        None,
        gio::DBusCallFlags::NONE,
        TIMEOUT,
    )
    .await
}

/// Player property `property` of player `name`.
async fn property(
    bus: &gio::DBusConnection,
    name: &str,
    property: &str,
) -> Result<glib::Variant, glib::Error> {
    let get = (PLAYER, property).to_variant();
    let reply = call(bus, name, PATH, PROPERTIES, "Get", Some(&get)).await?;
    // A `(v)` holding the value.
    Ok(reply
        .child_value(0)
        .as_variant()
        .unwrap_or_else(|| reply.child_value(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(action: &str) -> MediaAction {
        MediaAction {
            action: action.into(),
            query: None,
            service: None,
            direction: None,
            level: None,
            mute: None,
        }
    }

    #[test]
    fn transport_actions_are_player_methods() {
        assert_eq!(command(&media("pause")), Some(Command::Method("Pause")));
        assert_eq!(command(&media("resume")), Some(Command::Method("Play")));
        assert_eq!(command(&media("next")), Some(Command::Method("Next")));
        assert_eq!(
            command(&media("previous")),
            Some(Command::Method("Previous"))
        );
        assert_eq!(command(&media("stop")), Some(Command::Method("Stop")));
        assert_eq!(command(&media("play")), None);
        assert_eq!(command(&media("shuffle")), None);
    }

    #[test]
    fn volume_is_muting_first_then_a_level_then_a_direction() {
        let volume = |mute, level, direction: Option<&str>| {
            command(&MediaAction {
                mute,
                level,
                direction: direction.map(Into::into),
                ..media("volume")
            })
        };
        assert_eq!(volume(Some(true), Some(50), None), Some(Command::Mute));
        assert_eq!(volume(Some(false), None, None), Some(Command::Unmute));
        assert_eq!(
            volume(None, Some(40), Some("up")),
            Some(Command::Volume(0.4))
        );
        assert_eq!(volume(None, Some(140), None), Some(Command::Volume(1.0)));
        assert_eq!(volume(None, None, Some("up")), Some(Command::Louder(0.1)));
        assert_eq!(
            volume(None, None, Some("down")),
            Some(Command::Louder(-0.1))
        );
        assert_eq!(volume(None, None, None), None);
    }

    #[test]
    fn the_player_playing_is_picked_before_a_paused_one() {
        let players = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect()
        };
        let both = players(&[("a.paused", "Paused"), ("b.playing", "Playing")]);
        assert_eq!(pick(&both), Some("b.playing"));
        let paused = players(&[("a.stopped", "Stopped"), ("b.paused", "Paused")]);
        assert_eq!(pick(&paused), Some("b.paused"));
        assert_eq!(pick(&players(&[("a.stopped", "Stopped")])), None);
        assert_eq!(pick(&[]), None);
    }
}
