use std::sync::atomic::{AtomicBool, Ordering};

static MISSING_MODEL: AtomicBool = AtomicBool::new(false);
static SEGMENTATION_LOG: SegmentationLog = SegmentationLog;

struct SegmentationLog;

impl log::Log for SegmentationLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool { true }

    fn log(&self, record: &log::Record<'_>) {
        if record.args().to_string().contains("No segmentation model for complex script") {
            MISSING_MODEL.store(true, Ordering::SeqCst);
        }
    }

    fn flush(&self) {}
}

#[test]
fn complex_script_layout_has_models_and_wraps_without_data_errors() {
    log::set_logger(&SEGMENTATION_LOG).expect("install segmentation test logger");
    log::set_max_level(log::LevelFilter::Warn);
    let mut engine = incular::text::TextEngine::new();
    let style = incular::text::TextStyle::default();
    for text in [
        "千と千尋の神隠し。日本語の映画とテレビ番組を探します。",
        "你好世界。浏览电影和电视剧，发现新的精彩故事。",
        "ภาษาไทยสำหรับค้นหาภาพยนตร์และรายการโทรทัศน์",
    ] {
        let layout = engine.layout(text, &style, Some(80.0), incular::text::TextAlign::Start);
        assert!(layout.lines.len() > 1, "complex-script text should wrap: {text}");
        assert!(layout.metrics.size.height > 0.0);
        for line in layout.lines.iter() {
            assert!(text.is_char_boundary(line.start));
            assert!(text.is_char_boundary(line.end));
        }
    }
    assert!(!MISSING_MODEL.load(Ordering::SeqCst), "missing complex-script segmentation data");
}
