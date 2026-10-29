//! Which sound card PipeWire plays to by default. cpal's PipeWire host only
//! offers a "default" device with no card behind it, so this asks the
//! session manager's `default` metadata for the default sink's name and finds
//! that sink's `object.path` (`alsa:acp:<card id>:…` or `alsa:pcm:<card>:…`).

use pipewire as pw;
use pw::metadata::Metadata;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// How long to wait for PipeWire before giving up.
const TIMEOUT: Duration = Duration::from_millis(500);

/// Where the default sink is, as its card: an ALSA card id string or index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    Id(String),
    Index(u32),
}

/// The default sink's card, or None (no PipeWire, no default, not an ALSA card).
pub fn default_sink_card() -> Option<Card> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).ok()?;
    let context = pw::context::ContextRc::new(&mainloop, None).ok()?;
    let core = context.connect_rc(None).ok()?;
    let registry = core.get_registry_rc().ok()?;

    let default_name: Rc<RefCell<Option<String>>> = Rc::default();
    let paths: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let metas: Rc<RefCell<Vec<(Metadata, pw::metadata::MetadataListener)>>> = Rc::default();
    let pending = Rc::new(RefCell::new(vec![core.sync(0).ok()?]));

    let _core_listener = core
        .add_listener_local()
        .done({
            let (pending, mainloop) = (pending.clone(), mainloop.clone());
            move |id, seq| {
                let mut p = pending.borrow_mut();
                if id == pw::core::PW_ID_CORE
                    && let Some(i) = p.iter().position(|s| *s == seq)
                {
                    p.remove(i);
                    if p.is_empty() {
                        mainloop.quit();
                    }
                }
            }
        })
        .register();
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let (registry, core) = (registry.clone(), core.clone());
            let (default_name, paths, metas, pending) = (default_name.clone(), paths.clone(), metas.clone(), pending.clone());
            move |global| {
                let Some(props) = global.props else { return };
                match global.type_ {
                    ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
                        let Ok(meta) = registry.bind::<Metadata, _>(global) else { return };
                        let default_name = default_name.clone();
                        let listener = meta
                            .add_listener_local()
                            .property(move |_, key, _, value| {
                                if key == Some("default.audio.sink")
                                    && let Some(name) = value.and_then(json_name)
                                {
                                    *default_name.borrow_mut() = Some(name);
                                }
                                0
                            })
                            .register();
                        metas.borrow_mut().push((meta, listener));
                        // The properties arrive after another round trip.
                        if let Ok(seq) = core.sync(0) {
                            pending.borrow_mut().push(seq);
                        }
                    }
                    ObjectType::Node if props.get("media.class") == Some("Audio/Sink") => {
                        if let (Some(name), Some(path)) = (props.get("node.name"), props.get("object.path")) {
                            paths.borrow_mut().push((name.to_owned(), path.to_owned()));
                        }
                    }
                    _ => {}
                }
            }
        })
        .register();
    // Never hang on a server that doesn't answer.
    let timer = mainloop.loop_().add_timer({
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    let _ = timer.update_timer(Some(TIMEOUT), None);
    mainloop.run();

    let name = default_name.borrow().clone()?;
    let path = paths.borrow().iter().find(|(n, _)| *n == name).map(|(_, p)| p.clone())?;
    card_of_path(&path)
}

/// `{"name":"alsa_output.…"}` → the name.
fn json_name(value: &str) -> Option<String> {
    let rest = value.split("\"name\"").nth(1)?;
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    Some(rest[start..end].to_owned())
}

/// `alsa:acp:CODEC:4:playback` → id "CODEC"; `alsa:pcm:1:front:1:playback` → card 1.
fn card_of_path(path: &str) -> Option<Card> {
    let mut parts = path.split(':');
    if parts.next()? != "alsa" {
        return None;
    }
    match (parts.next()?, parts.next()?) {
        ("acp", id) => Some(Card::Id(id.to_owned())),
        ("pcm", n) => n.parse().ok().map(Card::Index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_paths_and_metadata_values() {
        assert_eq!(card_of_path("alsa:acp:CODEC:4:playback"), Some(Card::Id("CODEC".into())));
        assert_eq!(card_of_path("alsa:pcm:1:front:1:playback"), Some(Card::Index(1)));
        assert_eq!(card_of_path("bluez:something"), None);
        assert_eq!(json_name(r#"{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo"}"#).as_deref(), Some("alsa_output.pci-0000_00_1f.3.analog-stereo"));
        assert_eq!(json_name(r#"{ "name": "x" }"#).as_deref(), Some("x"));
    }
}
