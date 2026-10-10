//! The engine app worked as a person works it: clicks and keys on the real widgets, in a
//! window that is drawn but never shown.
//!
//! Needs a graphics card, so it only runs when asked, like mira's frame tests:
//!
//! ```sh
//! MIRA_FRAME_TESTS=1 cargo test -p mira_app --test editor
//! ```
//!
//! `MIRA_EDITOR_SHOT=<path>` also saves a picture of the window at the end.

use std::{sync::mpsc::Sender, time::Duration};

use mira::{
    live::Live,
    prelude::{Entity, Mesh3d, Parent, Transform},
};
use mira_app::editor::{
    agent::{Agent, Heard},
    Editor, Message, Placed,
};
use neo::testing::Harness;
use neo::App as _;
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
        Editor::new(game).with_first_layout().with_agent(Scripted),
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
    window.click(Point::new(25.0, 63.0));
    window.frame(TICK, 1.0);
    assert!(paused(&window), "Pause was pressed");
    window.click(Point::new(25.0, 63.0));
    window.frame(TICK, 1.0);
    assert!(!paused(&window), "Resume was pressed");

    // A row of the tree chooses its entity; the arrow keys move on from it.
    assert_eq!(window.app().chosen(), None);
    window.click(Point::new(900.0, 133.0));
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
    let field = Point::new(930.0, 507.0);
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

    // A row of the tree dragged onto another makes it that one's child. The child's parent
    // is then a field in the inspector, and a third row dropped on that field replaces it.
    let drag = |window: &mut Harness<Editor>, from: Point, to: Point| {
        window.event(Event::PointerMoved { pos: from });
        window.event(Event::PointerPressed {
            pos: from,
            button: PointerButton::Primary,
        });
        for step in 1..=8 {
            let along = step as f32 / 8.0;
            let pos = Point::new(
                from.x + (to.x - from.x) * along,
                from.y + (to.y - from.y) * along,
            );
            window.event(Event::PointerMoved { pos });
        }
        window.event(Event::PointerReleased {
            pos: to,
            button: PointerButton::Primary,
        });
        window.frame(TICK, 1.0);
        window.frame(TICK, 1.0);
    };
    let row = |index: usize| Point::new(900.0, 133.0 + 26.0 * index as f32);
    let parent_of = |window: &Harness<Editor>, child: Entity| {
        let world = &window.app().game().world;
        world.get::<Parent>(child).map(|parent| parent.0.index())
    };
    drag(&mut window, row(6), row(5));
    window.click(row(6));
    window.frame(TICK, 1.0);
    let child = window.app().chosen().expect("the row that was moved");
    assert_eq!((child.index(), parent_of(&window, child)), (6, Some(5)));
    drag(&mut window, row(4), Point::new(950.0, 652.0));
    assert_eq!(
        parent_of(&window, child),
        Some(4),
        "the row dropped on the field is the parent now"
    );

    // In the picture of the game, a press chooses what is under the pointer and a drag
    // slides it over the ground: the western site, pulled towards the camera.
    let site = Point::new(262.0, 310.0);
    window.click(site);
    window.frame(TICK, 1.0);
    let chosen = window.app().chosen().expect("the site under the pointer");
    assert_eq!(chosen.index(), 0, "the first site");
    let place = |window: &Harness<Editor>| {
        let world = &window.app().game().world;
        world.get::<Transform>(chosen).expect("a place").translation
    };
    let before = place(&window);
    drag(&mut window, site, Point::new(262.0, 391.0));
    let after = place(&window);
    assert!(
        after.z > before.z + 2.0 && (after.x - before.x).abs() < 0.2 && after.y == before.y,
        "from {before} to {after}"
    );

    // Further down the inspector, for the picture: the entity's colour.
    window.event(Event::Wheel {
        pos: Point::new(930.0, 600.0),
        delta: Point::new(0.0, 170.0),
    });
    window.frame(TICK, 1.0);

    // A cube is put in the scene from the Place panel's message, in the middle of the
    // picture, with a shape to be seen by; taken away again, the scene is as it was.
    let entities = window.app().game().world.entity_count();
    window.app_mut().update(Message::Place(Placed::Cube));
    window.frame(TICK, 1.0);
    let cube = window.app().chosen().expect("the new cube is chosen");
    assert!(window.app().game().world.get::<Mesh3d>(cube).is_some());
    assert_eq!(window.app().game().world.entity_count(), entities + 1);
    if let Ok(path) = std::env::var("MIRA_EDITOR_SHOT") {
        window.frame(TICK, 1.0);
        window
            .save_png(format!("{path}.placed.png"), 1.0)
            .expect("the picture saved");
    }
    window.app_mut().update(Message::Delete);
    window.frame(TICK, 1.0);
    assert_eq!(window.app().game().world.entity_count(), entities);

    // Every other panel opens from the Window menu and draws what it has to show.
    for panel in [
        "Log",
        "Console",
        "Profiler",
        "Systems",
        "World",
        "History",
        "Time",
        "Failures",
        "Plugins",
        "Statistics",
        "Signal graph",
        "Place",
    ] {
        if matches!(panel, "Log" | "Console" | "World" | "Place") {
            // Open already, behind another: brought to the front by being shut and opened.
            window.app_mut().update(Message::Panel(panel.to_owned()));
        }
        window.app_mut().update(Message::Panel(panel.to_owned()));
        window.frame(TICK, 1.0);
        let frame = window.frame(TICK, 1.0);
        assert_eq!(frame.len(), 1100 * 700 * 4, "{panel} drew");
        if let Ok(path) = std::env::var("MIRA_EDITOR_SHOT") {
            let path = format!("{path}.{panel}.png");
            window.save_png(path, 1.0).expect("the picture saved");
        }
        window.app_mut().update(Message::Panel(panel.to_owned()));
    }
    window.frame(TICK, 1.0);

    if let Ok(path) = std::env::var("MIRA_EDITOR_SHOT") {
        window.save_png(&path, 1.0).expect("the picture saved");
    }

    // The bar's Settings button opens the panel every Neo app has, over the rest.
    window.click(Point::new(1075.0, 63.0));
    window.frame(TICK, 1.0);
    assert!(window.app().settings_open());
    if let Ok(path) = std::env::var("MIRA_EDITOR_SHOT") {
        let path = format!("{path}.settings.png");
        window.save_png(path, 1.0).expect("the picture saved");
    }
}
