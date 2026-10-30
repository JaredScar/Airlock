//! Scripted screen scenes: an IDE with a `.env`, a DM, and an email window.
//! Secrets are pixelated immediately. Screen PII stays a suggestion until approved
//! (policy default). Ordinary lines are not redacted and their pixels stay put.

use airlock_core::{Category, Mode, Rect, Span, TextBox, Ts};
use airlock_media::video_fx::pixelate_bgra;
use airlock_pipeline::{Session, SessionConfig};

fn line(text: &str, y: f32) -> TextBox {
    TextBox {
        text: text.into(),
        rect: Rect {
            x: 0.05,
            y,
            w: 0.7,
            h: 0.05,
        },
        confidence: 0.95,
    }
}

#[test]
fn scripted_screens_blur_secrets_and_pii_only() {
    let mut session = Session::new(SessionConfig::default());
    let scenes = [
        (
            Span::new(Ts::ZERO, Ts::from_millis(250)),
            vec![
                line("DATABASE_URL=postgres://localhost/airlock", 0.02),
                line("AKIAIOSFODNN7EXAMPLE", 0.14),
            ],
        ),
        (
            Span::new(Ts::from_millis(250), Ts::from_millis(500)),
            vec![
                line("hey can you email me at jane.doe@example.com", 0.32),
                line("gg nice play", 0.46),
            ],
        ),
        (
            Span::new(Ts::from_millis(500), Ts::from_millis(750)),
            vec![
                line("From: studio@example.com", 0.60),
                line("Call me at 631-555-0142", 0.74),
                line("Subject: VOD ready", 0.88),
            ],
        ),
    ];
    session.on_live_time(Ts::from_millis(750));
    for (span, boxes) in &scenes {
        session.on_screen_text(boxes, *span);
    }

    let redactions = session.engine().all().to_vec();
    assert_eq!(redactions.len(), 4, "{redactions:?}");
    assert_eq!(
        redactions
            .iter()
            .filter(|r| r.category == Category::ScreenSecret)
            .count(),
        1
    );
    assert_eq!(
        redactions
            .iter()
            .filter(|r| r.category == Category::ScreenPii)
            .count(),
        3
    );
    let secret = redactions
        .iter()
        .find(|r| r.category == Category::ScreenSecret)
        .unwrap();
    assert!(secret.is_active());
    assert_eq!(secret.mode, Mode::Apply);
    let pii_ids: Vec<_> = redactions
        .iter()
        .filter(|r| r.category == Category::ScreenPii)
        .map(|r| {
            assert!(!r.is_active());
            r.id
        })
        .collect();
    for id in pii_ids {
        session.approve(id).unwrap();
    }
    assert!(session.engine().all().iter().all(|r| r.is_active()));

    let (w, h) = (40u32, 100u32);
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            frame[i] = if (x + y) % 2 == 0 { 0 } else { 255 };
            frame[i + 3] = 255;
        }
    }
    let before = frame.clone();
    for r in session.engine().all() {
        let airlock_core::Target::Video { region } = r.target else {
            panic!("screen redaction should be a region");
        };
        pixelate_bgra(&mut frame, w, h, w * 4, region, 4);
    }
    let benign = line("gg nice play", 0.46);
    let (x, y, bw, bh) = benign.rect.to_pixels(w, h);
    let i = ((y + bh / 2) * w + (x + bw / 2)) as usize * 4;
    assert_eq!(&frame[i..i + 4], &before[i..i + 4]);
    assert_ne!(frame, before);
}
