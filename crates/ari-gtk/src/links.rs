//! The web pages Ari opens for a search or a journey, from the search engine
//! and map the user picked in Preferences.

use gtk4::glib;

/// The search engines on offer: the `search-engine` setting's value, the
/// name shown for it, and its address with `{}` for the words.
pub const SEARCH_ENGINES: [(&str, &str, &str); 6] = [
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("google", "Google", "https://www.google.com/search?q={}"),
    ("bing", "Bing", "https://www.bing.com/search?q={}"),
    (
        "brave",
        "Brave Search",
        "https://search.brave.com/search?q={}",
    ),
    (
        "startpage",
        "Startpage",
        "https://www.startpage.com/do/search?q={}",
    ),
    ("ecosia", "Ecosia", "https://www.ecosia.org/search?q={}"),
];

/// The maps on offer, as [`SEARCH_ENGINES`], with `{}` for the destination.
pub const MAPS: [(&str, &str, &str); 2] = [
    (
        "openstreetmap",
        "OpenStreetMap",
        "https://www.openstreetmap.org/search?query={}",
    ),
    (
        "google",
        "Google Maps",
        "https://www.google.com/maps/dir/?api=1&destination={}",
    ),
];

/// The address for `words` on the choice named `setting` from `choices`, or
/// the first choice's when the setting names none of them.
pub fn link(choices: &[(&str, &str, &str)], setting: &str, words: &str) -> String {
    let (_, _, address) = choices
        .iter()
        .find(|(value, _, _)| *value == setting)
        .unwrap_or(&choices[0]);
    address.replace("{}", &glib::Uri::escape_string(words, None, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_goes_to_the_chosen_engine_with_its_words_escaped() {
        assert_eq!(
            link(&SEARCH_ENGINES, "google", "pecorino & figs"),
            "https://www.google.com/search?q=pecorino%20%26%20figs"
        );
        assert_eq!(
            link(&SEARCH_ENGINES, "duckduckgo", "caffè?"),
            "https://duckduckgo.com/?q=caff%C3%A8%3F"
        );
    }

    #[test]
    fn an_unknown_choice_falls_back_to_the_first() {
        assert_eq!(
            link(&SEARCH_ENGINES, "altavista", "cats"),
            "https://duckduckgo.com/?q=cats"
        );
        assert_eq!(
            link(&MAPS, "", "the airport"),
            "https://www.openstreetmap.org/search?query=the%20airport"
        );
    }

    #[test]
    fn directions_go_to_the_chosen_map() {
        assert_eq!(
            link(&MAPS, "google", "valletta/waterfront"),
            "https://www.google.com/maps/dir/?api=1&destination=valletta%2Fwaterfront"
        );
    }
}
