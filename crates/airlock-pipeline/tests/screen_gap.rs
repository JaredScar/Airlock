//! A secret that is on screen for one frame between OCR samples is still covered,
//! because the sample span runs back to the previous sample and policy pads it.

use airlock_core::{Category, Rect, Ts};
use airlock_media::video_fx::pixelate_bgra;
use airlock_pipeline::{Session, SessionConfig};
use airlock_vision::FrameSampler;

#[test]
fn a_key_between_samples_is_blurred() {
    let mut sampler = FrameSampler::new(4.0);
    assert!(sampler.offer(Ts::ZERO).is_some());
    let span = sampler.offer(Ts::from_millis(250)).unwrap();
    assert_eq!(span.start, Ts::ZERO);

    let mut session = Session::new(SessionConfig::default());
    session.on_live_time(Ts::from_millis(250));
    session.on_screen_text(
        &[airlock_core::TextBox {
            text: "live_123456789_AbCdEfGhIjKlMnOpQrStUvWx".into(),
            rect: Rect {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            },
            confidence: 0.95,
        }],
        span,
    );
    let redaction = session
        .engine()
        .all()
        .iter()
        .find(|r| r.category == Category::ScreenSecret)
        .expect("screen secret");
    let between = Ts::from_millis(100);
    assert!(
        redaction.span.contains(between),
        "frame at 100ms should be inside {:?}",
        redaction.span
    );

    let (w, h) = (8u32, 4u32);
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            frame[i] = if (x + y) % 2 == 0 { 0 } else { 255 };
            frame[i + 3] = 255;
        }
    }
    let before = frame.clone();
    let airlock_core::Target::Video { region } = redaction.target else {
        panic!("screen secret should cover a region");
    };
    pixelate_bgra(&mut frame, w, h, w * 4, region, 4);
    assert_ne!(frame, before);
}
