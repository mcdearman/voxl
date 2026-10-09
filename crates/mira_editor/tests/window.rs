//! The engine app worked as a person works it: clicks and keys on the real widgets, in a
//! window that is drawn but never shown.
//!
//! Needs a graphics card, so it only runs when asked, like mira's frame tests:
//!
//! ```sh
//! MIRA_FRAME_TESTS=1 cargo test -p mira_editor --test window
//! ```
//!
//! `MIRA_EDITOR_SHOT=<path>` also saves a picture of the window at the end.

use std::{sync::mpsc::Sender, time::Duration};

use mira::{live::Live, prelude::Transform};
use mira_editor::{
    agent::{Agent, Heard},
    Editor,
};
use neo::testing::Harness;
use neo::{Event, Key, Point, PointerButton, Size};

#[path = "../../../examples/sacred_sites/game.rs"]
mod game;

const TICK: Duration = Duration::from_millis(16);

/// An agent that answers anything the same way: some words, a look at the game, more words.
struct Scripted;

impl Agent for Scripted {
    fn ask(&mut self, _asked: &str, heard: Sender<Heard>) {
        let tool = || "look-1".to_owned();
        // A small picture, as a screenshot would come back.
        let picture: Vec<u8> = (0..48 * 27)
            .flat_map(|at| [(at % 48 * 5) as u8, 140, (at / 48 * 9) as u8, 255])
            .collect();
        for said in [
            Heard::Text("**Blue** is on a site, so the clock is stopped:\n\n".into()),
            Heard::Text("- `blue.contesting` is true\n- `red.clock.running` is false\n".into()),
            Heard::Tool {
                id: tool(),
                name: "mira_screenshot".into(),
            },
            Heard::Given {
                id: tool(),
                more: r#"{"width": 480}"#.into(),
            },
            Heard::Back {
                id: tool(),
                text: "the frame".into(),
                failed: false,
                picture: Some((48, 27, picture)),
            },
            Heard::Text("The clock is `red.clock`; it runs again when the scout leaves.".into()),
            Heard::Done,
        ] {
            let _ = heard.send(said);
        }
    }

    fn stop(&mut self) {}
}

#[test]
fn the_app_is_worked_by_clicking_on_it() {
    if !std::env::var("MIRA_FRAME_TESTS").is_ok_and(|asked| asked != "0") {
        eprintln!("skipped: set MIRA_FRAME_TESTS=1 on a machine with a graphics card");
        return;
    }
    let game = game::build(false).expect("the game");
    let mut window = Harness::new(
        Editor::new(game).with_agent(Scripted),
        Size::new(1100.0, 700.0),
    )
    .expect("a graphics card is needed for this test");
    let paused =
        |window: &Harness<Editor>| window.app().game().world.resource::<Live>().is_paused();
    for _ in 0..20 {
        window.frame(TICK, 1.0);
    }
    assert!(window.wants_frame(), "the game is drawn frame after frame");
    assert!(!paused(&window));

    // The game is in its panel: the middle of the window is the field's green, not the
    // window's own dark.
    let frame = window.frame(TICK, 1.0);
    let [r, g, b] = [0, 1, 2].map(|channel| frame[(400 * 1100 + 300) * 4 + channel]);
    assert!(
        g > r && g > b && g > 90,
        "the game's field, drawn: {r} {g} {b}"
    );

    // The bar's first button pauses the game, and then resumes it.
    window.click(Point::new(43.0, 73.0));
    window.frame(TICK, 1.0);
    assert!(paused(&window), "Pause was pressed");
    window.click(Point::new(43.0, 73.0));
    window.frame(TICK, 1.0);
    assert!(!paused(&window), "Resume was pressed");

    // A row of the tree chooses its entity; the arrow keys move on from it.
    assert_eq!(window.app().chosen(), None);
    window.click(Point::new(900.0, 149.0));
    window.frame(TICK, 1.0);
    let first = window
        .app()
        .chosen()
        .expect("a row of the tree was clicked");
    window.key(Key::Down, Default::default());
    window.frame(TICK, 1.0);
    let second = window.app().chosen().expect("still one chosen");
    assert_ne!(first, second, "Down moved to the next row");
    assert_eq!(second.index(), first.index() + 1);

    // The inspector shows the chosen entity's parts in fields. Dragging the first number of
    // its position to the right moves the entity in the game.
    let x = |window: &Harness<Editor>| {
        let world = &window.app().game().world;
        world
            .get::<Transform>(second)
            .expect("it has a place")
            .translation
            .x
    };
    let before = x(&window);
    let field = Point::new(920.0, 509.0);
    window.event(Event::PointerMoved { pos: field });
    window.event(Event::PointerPressed {
        pos: field,
        button: PointerButton::Primary,
    });
    for step in 1..=10 {
        let pos = Point::new(field.x + step as f32 * 4.0, field.y);
        window.event(Event::PointerMoved { pos });
    }
    window.event(Event::PointerReleased {
        pos: Point::new(field.x + 40.0, field.y),
        button: PointerButton::Primary,
    });
    window.frame(TICK, 1.0);
    let after = x(&window);
    assert!(
        (after - before - 1.0).abs() < 0.01,
        "ten steps of a tenth: from {before} to {after}"
    );

    // Something written in the agent's panel and sent with Enter joins the conversation,
    // and what the agent says and does comes after it.
    window.click(Point::new(350.0, 670.0));
    window.type_text("Why is the clock stopped?");
    window.frame(TICK, 1.0);
    window.key(Key::Enter, Default::default());
    window.frame(TICK, 1.0);
    window.frame(TICK, 1.0);
    let all = window.app().said();
    let said: Vec<&str> = all.iter().map(|said| said.text.as_str()).collect();
    assert_eq!(said.len(), 4, "{said:?}");
    assert_eq!(
        (said[0], said[2]),
        ("Why is the clock stopped?", "mira_screenshot")
    );
    assert!(said[1].starts_with("**Blue**") && said[3].contains("red.clock"));

    // Further down the inspector, for the picture: the entity's colour.
    window.event(Event::Wheel {
        pos: Point::new(930.0, 600.0),
        delta: Point::new(0.0, 170.0),
    });
    window.frame(TICK, 1.0);

    if let Ok(path) = std::env::var("MIRA_EDITOR_SHOT") {
        window.save_png(path, 1.0).expect("the picture saved");
    }
}
