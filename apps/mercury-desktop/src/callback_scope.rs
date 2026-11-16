//! Marks the code Mercury runs from Slint's callbacks, so that code reached from outside Slint
//! (an AppKit callback dispatched from a nested run loop, such as a modal panel's, opened by a
//! Slint callback) can tell that Slint is in the middle of a callback and may hold its state
//! borrowed.
//!
//! Every closure Mercury hands to Slint (window and UI callbacks, the rendering notifier, the
//! winit event hook, timers, `invoke_from_event_loop` closures) starts with
//! `let _slint = InSlintCallback::enter();`.

use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Held for the duration of one Slint callback.
pub(crate) struct InSlintCallback(());

impl InSlintCallback {
    pub(crate) fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self(())
    }
}

impl Drop for InSlintCallback {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Whether a Slint callback of Mercury's is running on this thread.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn inside_slint_callback() -> bool {
    DEPTH.with(|depth| depth.get() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_is_marked_while_a_callback_runs_including_nested_ones() {
        assert!(!inside_slint_callback());
        let outer = InSlintCallback::enter();
        {
            let _inner = InSlintCallback::enter();
            assert!(inside_slint_callback());
        }
        assert!(inside_slint_callback());
        drop(outer);
        assert!(!inside_slint_callback());
    }

    #[test]
    fn the_mark_is_removed_when_a_callback_unwinds() {
        let unwound = std::panic::catch_unwind(|| {
            let _scope = InSlintCallback::enter();
            panic!("deliberate panic inside a callback");
        });

        assert!(unwound.is_err());
        assert!(!inside_slint_callback());
    }
}

/// The convention above, checked over the source: every place where a closure or function is
/// handed to Slint must begin with `InSlintCallback::enter()`, unless the line before it (or the
/// registration's own line) says `// slint-callback: unguarded (<reason>)`.
#[cfg(test)]
mod convention {
    /// Registrations, as they appear in Mercury's code. `.on_` counts only when a closure
    /// follows (`.on_rendering_state(state, …)` is a method call).
    const REGISTRATIONS: [&str; 6] = [
        ".on_",
        "set_rendering_notifier(",
        "Timer::single_shot(",
        ".start(TimerMode",
        "invoke_from_event_loop(",
        "on_winit_window_event(",
    ];
    const GUARD: &str = "InSlintCallback::enter()";
    const EXEMPTION: &str = "slint-callback: unguarded (";

    /// `files`: (path, source). Returns "path:line" of every registration without the guard.
    pub(super) fn unguarded_registrations(files: &[(String, String)]) -> Vec<String> {
        let mut found = Vec::new();
        for (path, source) in files {
            let lines = source.lines().collect::<Vec<_>>();
            for (index, line) in lines.iter().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let Some((token, at)) = REGISTRATIONS
                    .iter()
                    .find_map(|token| line.find(token).map(|at| (*token, at)))
                else {
                    continue;
                };
                let after = &line[at + token.len()..];
                if token == ".on_" {
                    let Some(open) = after.find('(') else {
                        continue;
                    };
                    let name = &after[..open];
                    let arguments = after[open + 1..].trim_start();
                    let is_closure = arguments.starts_with('|') || arguments.starts_with("move");
                    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') || !is_closure {
                        continue;
                    }
                }
                let exempt =
                    line.contains(EXEMPTION) || index > 0 && lines[index - 1].contains(EXEMPTION);
                let rest = std::iter::once(after)
                    .chain(lines[index + 1..].iter().copied())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !exempt && !is_guarded(files, &rest) {
                    found.push(format!("{path}:{}", index + 1));
                }
            }
        }
        found
    }

    /// `rest`: the source from just after the registration's opening parenthesis.
    fn is_guarded(files: &[(String, String)], rest: &str) -> bool {
        let closure_bar = rest.find('|');
        let call_end = rest.find(')').unwrap_or(rest.len());
        match closure_bar {
            Some(bar) if bar < call_end => {
                let Some(body) = rest[bar..].find('{') else {
                    return false;
                };
                first_statement_is_guard(&rest[bar + body + 1..])
            }
            _ => {
                let argument = rest[..call_end].rsplit(',').next().unwrap_or("").trim();
                let name = argument.rsplit("::").next().unwrap_or(argument);
                !name.is_empty()
                    && files.iter().any(|(_, source)| {
                        source
                            .find(&format!("fn {name}("))
                            .and_then(|at| source[at..].find('{').map(|open| at + open + 1))
                            .is_some_and(|body| first_statement_is_guard(&source[body..]))
                    })
            }
        }
    }

    fn first_statement_is_guard(body: &str) -> bool {
        body.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .is_some_and(|line| line.contains(GUARD))
    }

    fn sources() -> Vec<(String, String)> {
        fn walk(directory: &std::path::Path, files: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(directory).expect("the source directory") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(&path, files);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && !path.ends_with("callback_scope.rs")
                {
                    let source = std::fs::read_to_string(&path).expect("a source file");
                    files.push((path.display().to_string(), source));
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        files
    }

    fn file(source: &str) -> Vec<(String, String)> {
        vec![("sample.rs".to_owned(), source.to_owned())]
    }

    #[test]
    fn every_slint_callback_in_the_source_is_marked() {
        let files = sources();
        assert!(files.len() > 10, "the source files were not found");
        assert_eq!(unguarded_registrations(&files), Vec::<String>::new());
    }

    #[test]
    fn an_unmarked_closure_is_reported() {
        let source = "fn f() {\n    window.on_reload(move || {\n        reload();\n    });\n}\n";
        assert_eq!(unguarded_registrations(&file(source)), vec!["sample.rs:2"]);
    }

    #[test]
    fn a_marked_closure_passes() {
        let source = "fn f() {\n    Timer::single_shot(delay, move || {\n        let _slint = \
                      crate::callback_scope::InSlintCallback::enter();\n        go();\n    });\n}\n";
        assert!(unguarded_registrations(&file(source)).is_empty());
    }

    #[test]
    fn a_named_function_must_begin_with_the_mark() {
        let marked = "fn f() {\n    slint::invoke_from_event_loop(pump);\n}\n\nfn pump() {\n    \
                      let _slint = InSlintCallback::enter();\n}\n";
        let unmarked = "fn f() {\n    slint::invoke_from_event_loop(pump);\n}\n\nfn pump() {\n    \
                        go();\n}\n";
        assert!(unguarded_registrations(&file(marked)).is_empty());
        assert_eq!(
            unguarded_registrations(&file(unmarked)),
            vec!["sample.rs:2"]
        );
    }

    #[test]
    fn an_exemption_with_a_reason_passes_and_method_calls_and_comments_are_ignored() {
        let source = "fn f() {\n    // slint-callback: unguarded (runs on Servo's thread)\n    \
                      waker.on_wake(|| {\n        post();\n    });\n    \
                      engine.on_rendering_state(state, api);\n    // invoke_from_event_loop(x)\n}\n";
        assert!(unguarded_registrations(&file(source)).is_empty());
    }
}
