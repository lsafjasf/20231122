use crate::{FrameRate, Language, SubtitleEvent, Timeline};

/// Demo project: a 120 s video, cues authored at 24/25/30 fps in Chinese,
/// English and Japanese, including two overlapping cues (zh#2/zh#3), one
/// suspicious gap and one cue past the end of the video.
pub fn demo_timeline() -> Timeline {
    let mut tl = Timeline::new(120_000_000_000);

    // zh @ 24fps
    tl.add_event(SubtitleEvent::new(
        1,
        Language::Zh,
        "你好，世界",
        0,
        72,
        FrameRate::FPS_24,
    ));
    tl.add_event(SubtitleEvent::new(
        2,
        Language::Zh,
        "今天天气不错",
        96,
        168,
        FrameRate::FPS_24,
    ));
    // Overlaps zh#2: starts at 6.25s while zh#2 runs until 7.00s.
    tl.add_event(SubtitleEvent::new(
        3,
        Language::Zh,
        "我们去散步吧",
        150,
        216,
        FrameRate::FPS_24,
    ));

    // en @ 25fps
    tl.add_event(SubtitleEvent::new(
        4,
        Language::En,
        "Hello, world",
        250,
        325,
        FrameRate::FPS_25,
    ));
    tl.add_event(SubtitleEvent::new(
        5,
        Language::En,
        "Nice weather today",
        350,
        450,
        FrameRate::FPS_25,
    ));

    // ja @ 30fps
    tl.add_event(SubtitleEvent::new(
        6,
        Language::Ja,
        "こんにちは",
        900,
        1080,
        FrameRate::FPS_30,
    ));
    // Past the 120 s video end: 130s - 133s.
    tl.add_event(SubtitleEvent::new(
        7,
        Language::Ja,
        "さようなら",
        3900,
        3990,
        FrameRate::FPS_30,
    ));

    tl
}
