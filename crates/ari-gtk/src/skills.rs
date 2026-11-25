//! The Skills page: the installed skills and the registry's, and a dialog per
//! skill to install, update or remove it and change its settings.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ari_runtime::skills::{BrowseEntry, SkillManifest, SkillRegistry, SkillRegistryError};
use gettextrs::gettext;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::warn;

use crate::ari::Ari;
use crate::fields::{self, Changed};
use crate::markdown;
use crate::providers::language_name;

/// One row of either list.
#[derive(Debug, Clone)]
struct Skill {
    id: String,
    name: String,
    description: String,
    /// The installed one's, or else the registry's.
    version: String,
    installed: bool,
    /// The registry's newer version, when there is one.
    update: Option<String>,
    /// What this computer can't provide for it, by capability name.
    missing: Vec<String>,
    /// The languages it speaks. None listed means any.
    languages: Vec<String>,
    /// An assistant rather than a skill.
    assistant: bool,
    author: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    /// The SKILL.md body, for an installed skill.
    body: Option<String>,
}

/// What to do once a change to the installed skills has worked.
type AfterChange = Box<dyn FnOnce(&Rc<SkillsPage>)>;

pub struct SkillsPage {
    pub root: gtk4::Box,
    ari: Rc<Ari>,
    toggles: adw::ToggleGroup,
    list: gtk4::ListBox,
    status: gtk4::Label,
    spinner: adw::Spinner,
    toasts: adw::ToastOverlay,
    /// Bumped by each refresh, so a slow one that's been overtaken drops
    /// what it fetched.
    generation: RefCell<u64>,
    /// The skill to open once the registry's list is in.
    opening: RefCell<Option<String>>,
}

impl SkillsPage {
    pub fn new(ari: &Rc<Ari>, toasts: &adw::ToastOverlay) -> Rc<Self> {
        let installed = adw::Toggle::builder()
            .label(gettext("Installed"))
            .name("installed")
            .build();
        let browse = adw::Toggle::builder()
            .label(gettext("Browse"))
            .name("browse")
            .build();
        let toggles = adw::ToggleGroup::builder()
            .halign(gtk4::Align::Center)
            .build();
        toggles.add(installed);
        toggles.add(browse);
        toggles.set_active_name(Some("installed"));

        let status = gtk4::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .css_classes(["dim-label"])
            .build();
        let spinner = adw::Spinner::builder()
            .height_request(32)
            .visible(false)
            .build();
        let list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(18)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        column.append(&toggles);
        column.append(&spinner);
        column.append(&status);
        column.append(&list);
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(720)
                    .child(&column)
                    .build(),
            )
            .build();
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&scroller);

        let page = Rc::new(Self {
            root,
            ari: ari.clone(),
            toggles: toggles.clone(),
            list,
            status,
            spinner,
            toasts: toasts.clone(),
            generation: RefCell::new(0),
            opening: RefCell::default(),
        });
        toggles.connect_active_name_notify(glib::clone!(
            #[weak]
            page,
            move |_| page.refresh()
        ));
        page
    }

    /// Show skill `id` from the registry, opened.
    pub fn show_skill(self: &Rc<Self>, id: &str) {
        *self.opening.borrow_mut() = Some(id.to_string());
        if self.browsing() {
            self.refresh();
        } else {
            self.toggles.set_active_name(Some("browse"));
        }
    }

    fn browsing(&self) -> bool {
        self.toggles.active_name().as_deref() == Some("browse")
    }

    /// Fetch the list on show again: the installed skills and their updates,
    /// or the registry's.
    pub fn refresh(self: &Rc<Self>) {
        let generation = {
            let mut generation = self.generation.borrow_mut();
            *generation += 1;
            *generation
        };
        self.spinner.set_visible(true);
        self.status.set_visible(false);
        let registry = self.ari.engine.skills.clone();
        let locale = self.ari.engine.language();
        let browsing = self.browsing();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let fetched = gio::spawn_blocking(move || {
                    if browsing {
                        browse(&registry)
                    } else {
                        installed(&registry, &locale)
                    }
                })
                .await;
                if *page.generation.borrow() != generation {
                    return;
                }
                page.spinner.set_visible(false);
                match fetched {
                    Ok(Ok((skills, note))) => {
                        page.show(&skills);
                        page.show_status(note);
                        let opening = page.opening.take();
                        if let Some(skill) =
                            opening.and_then(|id| skills.iter().find(|s| s.id == id))
                        {
                            page.open(skill);
                        }
                    }
                    Ok(Err(error)) => {
                        page.show(&[]);
                        page.show_status(Some(describe(&error)));
                    }
                    Err(_) => warn!("fetching the skills panicked"),
                }
            }
        ));
    }

    fn show_status(&self, note: Option<String>) {
        self.status.set_visible(note.is_some());
        self.status.set_label(note.as_deref().unwrap_or_default());
    }

    fn show(self: &Rc<Self>, skills: &[Skill]) {
        self.list.remove_all();
        self.list.set_visible(!skills.is_empty());
        for skill in skills {
            self.list.append(&self.row(skill));
        }
        if skills.is_empty() && self.status.label().is_empty() {
            self.show_status(Some(if self.browsing() {
                gettext("The registry has no skills.")
            } else {
                gettext("No skills installed yet. Find some under Browse.")
            }));
        }
    }

    fn row(self: &Rc<Self>, skill: &Skill) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&skill.name)
            .subtitle(&skill.description)
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_prefix(
            &adw::Avatar::builder()
                .size(36)
                .text(&skill.name)
                .show_initials(true)
                .build(),
        );
        if !skill.missing.is_empty() {
            row.set_subtitle(&needs(&skill.missing));
            row.set_activatable(false);
            row.add_css_class("dim-label");
            return row;
        }
        let suffix: gtk4::Widget = match (skill.installed, &skill.update) {
            (true, Some(_)) => {
                let button = gtk4::Button::builder()
                    .label(gettext("Update"))
                    .valign(gtk4::Align::Center)
                    .build();
                let (id, name) = (skill.id.clone(), skill.name.clone());
                button.connect_clicked(glib::clone!(
                    #[weak(rename_to = page)]
                    self,
                    move |button| page.update(&id, &name, button)
                ));
                button.upcast()
            }
            (true, None) => gtk4::Label::builder()
                .label(if self.browsing() {
                    gettext("Installed")
                } else {
                    skill.version.clone()
                })
                .css_classes(["dim-label"])
                .build()
                .upcast(),
            (false, _) => {
                let button = gtk4::Button::builder()
                    .label(gettext("Install"))
                    .valign(gtk4::Align::Center)
                    .css_classes(["suggested-action"])
                    .build();
                let skill = skill.clone();
                button.connect_clicked(glib::clone!(
                    #[weak(rename_to = page)]
                    self,
                    move |button| page.install(&skill, button)
                ));
                button.upcast()
            }
        };
        row.add_suffix(&suffix);
        let skill = skill.clone();
        row.connect_activated(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.open(&skill)
        ));
        row
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// Run `call` on the registry off the main loop, then load the installed
    /// skills into the engine again and redraw. `done` says what happened;
    /// `then` runs after, when it worked.
    fn change(
        self: &Rc<Self>,
        call: impl FnOnce(&SkillRegistry) -> Result<(), SkillRegistryError> + Send + 'static,
        done: String,
        busy: &gtk4::Widget,
        then: Option<AfterChange>,
    ) {
        busy.set_sensitive(false);
        let registry = self.ari.engine.skills.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[weak]
            busy,
            async move {
                let result = gio::spawn_blocking(move || call(&registry)).await;
                busy.set_sensitive(true);
                match result {
                    Ok(Ok(())) => {
                        if page.ari.engine.reload_skills().await.is_err() {
                            warn!("loading the installed skills panicked");
                        }
                        page.toast(&done);
                        page.refresh();
                        if let Some(then) = then {
                            then(&page);
                        }
                    }
                    Ok(Err(error)) => page.toast(&describe(&error)),
                    Err(_) => warn!("a skill store call panicked"),
                }
            }
        ));
    }

    /// Install `skill`, asking first when it doesn't speak Ari's language.
    fn install(self: &Rc<Self>, skill: &Skill, button: &impl IsA<gtk4::Widget>) {
        let language = self.ari.engine.language();
        if speaks(&skill.languages, &language) {
            self.start_install(skill, button.upcast_ref());
            return;
        }
        let languages: Vec<String> = skill.languages.iter().map(|l| language_name(l)).collect();
        let confirm = adw::AlertDialog::builder()
            .heading(gettext("Language Not Supported"))
            .body(
                gettext("{skill} isn't available in {language}. It speaks {languages}, and answers in that. Install it anyway?")
                    .replace("{skill}", &skill.name)
                    .replace("{language}", &language_name(&language))
                    .replace("{languages}", &languages.join(", ")),
            )
            .close_response("cancel")
            .default_response("install")
            .build();
        confirm.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("install", &gettext("Install Anyway")),
        ]);
        confirm.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        let skill = skill.clone();
        let busy = button.clone().upcast::<gtk4::Widget>();
        confirm.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |_, response| {
                    if response == "install" {
                        page.start_install(&skill, &busy);
                    }
                }
            ),
        );
        confirm.present(Some(&self.root));
    }

    fn start_install(self: &Rc<Self>, skill: &Skill, busy: &gtk4::Widget) {
        let id = skill.id.clone();
        let done = gettext("{} installed").replace("{}", &skill.name);
        let then = skill.assistant.then(|| {
            let (id, name) = (skill.id.clone(), skill.name.clone());
            Box::new(move |page: &Rc<Self>| page.offer_assistant(&id, &name)) as AfterChange
        });
        self.change(
            move |registry| registry.install_skill_by_id(&id).map(|_| ()),
            done,
            busy,
            then,
        );
    }

    /// A newly installed assistant, when none answers yet: offer it the job.
    fn offer_assistant(self: &Rc<Self>, id: &str, name: &str) {
        let settings = &self.ari.settings;
        let listed = self
            .ari
            .engine
            .assistants
            .list_assistants()
            .iter()
            .any(|a| a.id == id && a.provider == "api");
        if !settings.string("assistant").is_empty() || !listed {
            return;
        }
        let offer = adw::AlertDialog::builder()
            .heading(gettext("Use {} as Your Assistant?").replace("{}", name))
            .body(
                gettext(
                    "{} can answer whatever no skill does. You can change this in Preferences.",
                )
                .replace("{}", name),
            )
            .close_response("later")
            .default_response("use")
            .build();
        offer.add_responses(&[
            ("later", &gettext("Not Now")),
            ("use", &gettext("Use {}").replace("{}", name)),
        ]);
        offer.set_response_appearance("use", adw::ResponseAppearance::Suggested);
        let id = id.to_string();
        offer.connect_response(
            None,
            glib::clone!(
                #[weak]
                settings,
                move |_, response| {
                    if response == "use" {
                        let _ = settings.set_string("assistant", &id);
                    }
                }
            ),
        );
        offer.present(Some(&self.root));
    }

    fn update(self: &Rc<Self>, id: &str, name: &str, button: &impl IsA<gtk4::Widget>) {
        let id = id.to_string();
        let done = gettext("{} updated").replace("{}", name);
        self.change(
            move |registry| registry.install_skill_update(&id).map(|_| ()),
            done,
            button.upcast_ref(),
            None,
        );
    }

    fn remove(self: &Rc<Self>, skill: &Skill, dialog: &adw::PreferencesDialog) {
        let confirm = adw::AlertDialog::builder()
            .heading(gettext("Remove {}?").replace("{}", &skill.name))
            .body(gettext("Its settings and anything it stored go with it."))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        confirm.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("remove", &gettext("Remove")),
        ]);
        confirm.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let id = skill.id.clone();
        let done = gettext("{} removed").replace("{}", &skill.name);
        confirm.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = page)]
                self,
                #[weak]
                dialog,
                move |_, response| {
                    if response != "remove" {
                        return;
                    }
                    dialog.close();
                    let id = id.clone();
                    page.change(
                        move |registry| registry.uninstall_skill_by_id(&id),
                        done.clone(),
                        page.list.upcast_ref(),
                        None,
                    );
                }
            ),
        );
        confirm.present(Some(dialog));
    }

    /// The skill's own dialog: what it is, its settings when it's installed,
    /// and the buttons to install, update or remove it.
    fn open(self: &Rc<Self>, skill: &Skill) {
        let dialog = adw::PreferencesDialog::builder()
            .title(&skill.name)
            .content_width(560)
            .build();
        let page = adw::PreferencesPage::new();

        let about = adw::PreferencesGroup::builder()
            .description(&skill.description)
            .build();
        if let Some(body) = skill.body.as_deref().filter(|b| !b.trim().is_empty()) {
            about.add(&markdown::widget(body, 60));
        }
        page.add(&about);

        let info = adw::PreferencesGroup::new();
        let facts = [
            (gettext("Version"), Some(skill.version.clone())),
            (gettext("Author"), skill.author.clone()),
            (gettext("Licence"), skill.license.clone()),
        ];
        for (title, value) in facts {
            if let Some(value) = value {
                info.add(
                    &adw::ActionRow::builder()
                        .title(title)
                        .subtitle(value)
                        .subtitle_selectable(true)
                        .css_classes(["property"])
                        .build(),
                );
            }
        }
        if let Some(homepage) = &skill.homepage {
            let row = adw::ActionRow::builder()
                .title(gettext("Website"))
                .subtitle(homepage)
                .activatable(true)
                .css_classes(["property"])
                .build();
            row.add_suffix(&gtk4::Image::from_icon_name("adw-external-link-symbolic"));
            let homepage = homepage.clone();
            row.connect_activated(move |row| {
                gtk4::UriLauncher::new(&homepage).launch(
                    row.root().and_downcast_ref::<gtk4::Window>(),
                    gio::Cancellable::NONE,
                    |result| {
                        if let Err(error) = result {
                            warn!(%error, "couldn't open the skill's website");
                        }
                    },
                );
            });
            info.add(&row);
        }
        page.add(&info);

        if skill.installed {
            let settings = gtk4::ListBox::builder()
                .selection_mode(gtk4::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            let group = adw::PreferencesGroup::builder()
                .title(gettext("Settings"))
                .visible(false)
                .build();
            group.add(&settings);
            page.add(&group);
            self.fill_settings(&group, &settings, &skill.id, &dialog);
        }

        let actions = adw::PreferencesGroup::new();
        let buttons = gtk4::Box::builder()
            .spacing(12)
            .halign(gtk4::Align::Center)
            .build();
        if !skill.installed {
            let install = gtk4::Button::builder()
                .label(gettext("Install"))
                .css_classes(["pill", "suggested-action"])
                .build();
            let skill = skill.clone();
            install.connect_clicked(glib::clone!(
                #[weak(rename_to = page)]
                self,
                #[weak]
                dialog,
                move |button| {
                    dialog.close();
                    page.install(&skill, button);
                }
            ));
            buttons.append(&install);
        } else {
            if skill.update.is_some() {
                let update = gtk4::Button::builder()
                    .label(gettext("Update"))
                    .css_classes(["pill", "suggested-action"])
                    .build();
                let (id, name) = (skill.id.clone(), skill.name.clone());
                update.connect_clicked(glib::clone!(
                    #[weak(rename_to = page)]
                    self,
                    #[weak]
                    dialog,
                    move |button| {
                        page.update(&id, &name, button);
                        dialog.close();
                    }
                ));
                buttons.append(&update);
            }
            let remove = gtk4::Button::builder()
                .label(gettext("Remove"))
                .css_classes(["pill", "destructive-action"])
                .build();
            let skill = skill.clone();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = page)]
                self,
                #[weak]
                dialog,
                move |_| page.remove(&skill, &dialog)
            ));
            buttons.append(&remove);
        }
        actions.add(&buttons);
        page.add(&actions);

        dialog.add(&page);
        dialog.present(self.root.root().as_ref());
    }

    /// The installed skill's settings in `list`, drawn again after each
    /// change. `group` shows only when there are some.
    fn fill_settings(
        self: &Rc<Self>,
        group: &adw::PreferencesGroup,
        list: &gtk4::ListBox,
        id: &str,
        dialog: &adw::PreferencesDialog,
    ) {
        let registry = self.ari.engine.skills.clone();
        let (id, locale) = (id.to_string(), self.ari.engine.language());
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[weak]
            group,
            #[weak]
            list,
            #[weak]
            dialog,
            async move {
                let lookup = id.clone();
                let fields =
                    gio::spawn_blocking(move || registry.get_skill_settings(&lookup, &locale))
                        .await;
                let fields = match fields {
                    Ok(Ok(fields)) => fields,
                    Ok(Err(error)) => {
                        dialog.add_toast(adw::Toast::new(&describe(&error)));
                        return;
                    }
                    Err(_) => return,
                };
                group.set_visible(!fields.is_empty());
                list.remove_all();
                let changed = Rc::new(Changed {
                    failed: Box::new(glib::clone!(
                        #[weak]
                        dialog,
                        move |error| {
                            dialog.add_toast(adw::Toast::new(&saving_failed(&error)));
                        }
                    )),
                    saved: Box::new(glib::clone!(
                        #[weak]
                        page,
                        #[weak]
                        group,
                        #[weak]
                        list,
                        #[weak]
                        dialog,
                        #[strong]
                        id,
                        move || page.fill_settings(&group, &list, &id, &dialog)
                    )),
                });
                for row in fields::rows(&page.ari, &id, &fields, changed) {
                    list.append(&row);
                }
            }
        ));
    }
}

pub fn saving_failed(error: &str) -> String {
    gettext("Couldn't save this: {}").replace("{}", error)
}

/// The installed skills, with the updates the registry has for them. Updates
/// that can't be checked leave the list as it is, with a note.
fn installed(
    registry: &SkillRegistry,
    locale: &str,
) -> Result<(Vec<Skill>, Option<String>), SkillRegistryError> {
    let (updates, note) = match registry.check_for_updates() {
        Ok(updates) => (
            updates
                .into_iter()
                .map(|u| (u.id, u.available_version))
                .collect::<HashMap<_, _>>(),
            None,
        ),
        Err(error) => (
            HashMap::new(),
            Some(gettext("Couldn't check for updates: {}").replace("{}", &describe(&error))),
        ),
    };
    let skills = registry
        .list_installed()
        .into_iter()
        .map(|installed| {
            let manifest = registry.read_installed_manifest(&installed.id, locale);
            if let Err(error) = &manifest {
                warn!(%error, id = installed.id, "couldn't read an installed skill's manifest");
            }
            let manifest = manifest.ok();
            Skill {
                name: manifest
                    .as_ref()
                    .map_or_else(|| installed.id.clone(), |m| m.name.clone()),
                description: manifest
                    .as_ref()
                    .map(|m| m.description.clone())
                    .unwrap_or_default(),
                update: updates.get(&installed.id).cloned(),
                version: installed.version,
                installed: true,
                missing: Vec::new(),
                languages: installed.languages.clone(),
                assistant: false,
                author: manifest.as_ref().and_then(|m| m.author.clone()),
                homepage: manifest.as_ref().and_then(|m| m.homepage.clone()),
                license: manifest.as_ref().and_then(|m| m.license.clone()),
                body: manifest.map(|m: SkillManifest| m.body),
                id: installed.id,
            }
        })
        .collect();
    Ok((skills, note))
}

fn browse(registry: &SkillRegistry) -> Result<(Vec<Skill>, Option<String>), SkillRegistryError> {
    let skills = registry
        .browse_registry()?
        .into_iter()
        .map(|entry: BrowseEntry| Skill {
            version: entry.version,
            installed: entry.installed,
            update: None,
            missing: entry.missing_capabilities,
            languages: entry.languages,
            assistant: entry.skill_type == "assistant",
            author: entry.author,
            homepage: entry.homepage,
            license: entry.license,
            body: None,
            name: entry.name,
            description: entry.description,
            id: entry.id,
        })
        .collect();
    Ok((skills, None))
}

/// Whether a skill listing `languages` speaks `language`. One that lists
/// none speaks any.
fn speaks(languages: &[String], language: &str) -> bool {
    languages.is_empty() || languages.iter().any(|l| l.eq_ignore_ascii_case(language))
}

/// Why a skill store call failed, in words for the user.
pub fn describe(error: &SkillRegistryError) -> String {
    match error {
        SkillRegistryError::Network { .. } => {
            gettext("Couldn't reach the skill registry. Check your connection.")
        }
        SkillRegistryError::BadStatus { .. } => {
            gettext("The skill registry isn't available right now. Try again later.")
        }
        SkillRegistryError::TooLarge { .. } => gettext("This skill is too large to install."),
        SkillRegistryError::Integrity { .. } => {
            gettext("This skill couldn't be verified, so it wasn't installed.")
        }
        SkillRegistryError::Registry { .. } => {
            gettext("Couldn't read the skill registry. Try again.")
        }
        SkillRegistryError::NotFound { .. } => gettext("The registry no longer has that skill."),
        SkillRegistryError::NotInstalled { .. } => gettext("That skill isn't installed."),
        SkillRegistryError::Manifest { .. } | SkillRegistryError::ManifestUnavailable { .. } => {
            gettext("Couldn't read the skill's description.")
        }
        SkillRegistryError::TrustKey { .. } => {
            gettext("Ari's signing key is damaged. Reinstall Ari.")
        }
        SkillRegistryError::Store { message } => {
            gettext("Couldn't change the installed skills: {}").replace("{}", message)
        }
    }
}

/// What a skill needs that this computer can't do, in words for the user.
fn needs(missing: &[String]) -> String {
    let names: Vec<String> = missing.iter().map(|name| capability(name)).collect();
    gettext("Not available in Ari for Linux. Needs {}.").replace("{}", &names.join(", "))
}

fn capability(name: &str) -> String {
    match name {
        "http" => gettext("the internet"),
        "location" => gettext("your location"),
        "notifications" => gettext("notifications"),
        "launch_app" => gettext("opening apps"),
        "clipboard" => gettext("the clipboard"),
        "tts" => gettext("speaking aloud"),
        "storage_kv" => gettext("storage"),
        "calendar" => gettext("your calendar"),
        "tasks" => gettext("your task lists"),
        "authorize" => gettext("signing in to an account"),
        "media_control" => gettext("media controls"),
        "media_services" => gettext("music services"),
        "critical_alert" => gettext("alerts that ring"),
        "alarm" => gettext("alarms"),
        "navigation" => gettext("directions"),
        "send_message" => gettext("sending messages"),
        "contacts" => gettext("your contacts"),
        "reply" => gettext("replying to messages"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn a_skill_speaks_a_language_it_lists() {
        assert!(speaks(&langs(&["en", "it"]), "it"));
        assert!(speaks(&langs(&["EN"]), "en"));
    }

    #[test]
    fn a_skill_does_not_speak_a_language_it_leaves_out() {
        assert!(!speaks(&langs(&["en"]), "it"));
    }

    #[test]
    fn a_skill_listing_no_languages_speaks_any() {
        assert!(speaks(&[], "it"));
    }
}
