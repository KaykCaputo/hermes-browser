//! Quitting while downloads are running: ask first.

/// The prompt shown before quitting, if one is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuitPrompt {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm: String,
    pub(crate) keep: String,
}

/// `None` (quit at once) without active downloads; otherwise the question to ask.
pub(crate) fn quit_prompt(active_downloads: usize) -> Option<QuitPrompt> {
    match active_downloads {
        0 => None,
        1 => Some(QuitPrompt {
            title: "Cancel 1 download and quit?".to_owned(),
            body: "If the server allows it, you can resume it from the downloads list the next \
                   time you open Mercury Browser."
                .to_owned(),
            confirm: "Cancel download and quit".to_owned(),
            keep: "Keep downloading".to_owned(),
        }),
        count => Some(QuitPrompt {
            title: format!("Cancel {count} downloads and quit?"),
            body: "If the servers allow it, you can resume them from the downloads list the \
                   next time you open Mercury Browser."
                .to_owned(),
            confirm: "Cancel downloads and quit".to_owned(),
            keep: "Keep downloading".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_asked_without_active_downloads() {
        assert_eq!(quit_prompt(0), None);
    }

    #[test]
    fn active_downloads_are_counted_in_the_question() {
        let one = quit_prompt(1).unwrap();
        assert_eq!(one.title, "Cancel 1 download and quit?");
        assert_eq!(one.confirm, "Cancel download and quit");
        assert_eq!(one.keep, "Keep downloading");
        assert!(one.body.contains("resume"), "{}", one.body);

        let three = quit_prompt(3).unwrap();
        assert_eq!(three.title, "Cancel 3 downloads and quit?");
        assert_eq!(three.confirm, "Cancel downloads and quit");
    }
}
