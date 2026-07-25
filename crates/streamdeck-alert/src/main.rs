//! `streamdeck-alert` — a minimal always-on-top completion alert.
//!
//! Native AppKit only: no WebKit, no bundle, no persistent state. The process
//! exists only while a Pomodoro completion is pending. It prints `start` or
//! `dismiss` on stdout and exits, which is how the daemon learns the outcome.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "streamdeck-alert",
    version,
    about = "Show a modal Pomodoro completion alert and report the choice"
)]
struct Cli {
    #[arg(long, default_value = "Pomodoro")]
    title: String,

    #[arg(long)]
    message: String,

    /// Label for the button that starts the next phase.
    #[arg(long, default_value = "Start")]
    primary: String,

    /// Label for the button that only dismisses the alert.
    #[arg(long, default_value = "Dismiss")]
    dismiss: String,
}

/// What the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Start,
    Dismiss,
}

impl Choice {
    /// The single word the daemon reads from stdout.
    const fn token(self) -> &'static str {
        match self {
            Choice::Start => "start",
            Choice::Dismiss => "dismiss",
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let choice = show(&cli);
    println!("{}", choice.token());
}

#[cfg(target_os = "macos")]
fn show(cli: &Cli) -> Choice {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{
        NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationPolicy, NSWindowLevel,
    };
    use objc2_foundation::{MainThreadMarker, NSString};

    // AppKit requires the main thread; this binary has only one.
    let Some(marker) = MainThreadMarker::new() else {
        return Choice::Dismiss;
    };

    autoreleasepool(|_| {
        let application = NSApplication::sharedApplication(marker);
        // `Accessory` keeps the alert out of the Dock but still lets it come
        // forward, which is what makes it hard to miss without being a real app.
        application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        application.activate();

        let alert = NSAlert::new(marker);
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.setMessageText(&NSString::from_str(&cli.title));
        alert.setInformativeText(&NSString::from_str(&cli.message));
        // The first button added is the default, activated by Return.
        alert.addButtonWithTitle(&NSString::from_str(&cli.primary));
        alert.addButtonWithTitle(&NSString::from_str(&cli.dismiss));
        // Float above full-screen apps so a pending completion is visible.
        alert.window().setLevel(NSWindowLevel::from(4isize));

        // `NSAlertFirstButtonReturn` is 1000; anything else is the dismiss button.
        const FIRST_BUTTON: isize = 1000;
        let response = alert.runModal();
        if response == objc2_app_kit::NSModalResponse::from(FIRST_BUTTON) {
            Choice::Start
        } else {
            Choice::Dismiss
        }
    })
}

#[cfg(not(target_os = "macos"))]
fn show(_cli: &Cli) -> Choice {
    // On any other platform the daemon falls back to the deck's alert state.
    Choice::Dismiss
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tokens_are_the_words_the_daemon_parses() {
        assert_eq!(Choice::Start.token(), "start");
        assert_eq!(Choice::Dismiss.token(), "dismiss");
    }

    #[test]
    fn the_cli_takes_the_labels_the_daemon_passes() {
        let cli = Cli::parse_from([
            "streamdeck-alert",
            "--title",
            "Pomodoro",
            "--message",
            "Focus complete. Your 5-minute break is ready.",
            "--primary",
            "Start Break",
        ]);
        assert_eq!(cli.title, "Pomodoro");
        assert_eq!(cli.message, "Focus complete. Your 5-minute break is ready.");
        assert_eq!(cli.primary, "Start Break");
        assert_eq!(cli.dismiss, "Dismiss");
    }

    #[test]
    fn a_message_is_required_so_a_blank_alert_can_never_appear() {
        assert!(Cli::try_parse_from(["streamdeck-alert"]).is_err());
    }
}
