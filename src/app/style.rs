use gtk4 as gtk;

const CSS: &str = r#"
window.app-window {
    background: #0a0f14;
    color: #eaf3f8;
    font-family: "Noto Sans CJK SC", "Noto Sans", "DejaVu Sans", sans-serif;
    font-size: 13px;
}
headerbar.app-header {
    background: #0d141b;
    border-bottom: 1px solid #243545;
    box-shadow: none;
    min-height: 62px;
    padding: 0 16px;
}
headerbar.app-header windowcontrols button {
    background: transparent;
    border: none;
}
.app-title {
    font-size: 17px;
    font-weight: 800;
    color: #f3f8fb;
    letter-spacing: 0.4px;
}
.app-subtitle {
    font-size: 10px;
    color: #7f97a9;
    letter-spacing: 1.2px;
}
.app-root {
    background: #0a0f14;
}
.control-panel {
    background: #0d141b;
    border-right: 1px solid #223444;
    padding: 18px;
}
.detail-panel {
    background: #0a0f14;
    padding: 22px 26px 36px 26px;
}
.eyebrow {
    color: #6f899c;
    font-size: 10px;
    font-weight: 800;
    letter-spacing: 1.6px;
}
.section-title {
    color: #d8e8f2;
    font-size: 14px;
    font-weight: 800;
    letter-spacing: 0.3px;
}
.card {
    background: #111a23;
    border: 1px solid #263848;
    border-radius: 10px;
    padding: 13px;
}
.device-card {
    background: #111a23;
    border: 1px solid #293d4e;
    border-radius: 9px;
    padding: 12px;
}
.source-card {
    background: #101923;
    border: 1px solid #294052;
    border-radius: 10px;
    padding: 15px;
}
.attempt-card {
    background: #101820;
    border: 1px solid #233646;
    border-radius: 9px;
    padding: 11px 13px;
}
.attempt-card-selected {
    border: 1px solid #c98b36;
    background: #1a1a18;
}
.compare-card {
    background: #0f1b1a;
    border: 1px solid #24504b;
    border-radius: 10px;
    padding: 13px;
}
.status-strip {
    background: #0e161e;
    border-bottom: 1px solid #223444;
    padding: 9px 22px;
}
.detail-title {
    font-size: 30px;
    font-weight: 800;
    color: #f6fafc;
    letter-spacing: -0.5px;
}
.metric {
    font-size: 19px;
    font-weight: 800;
    color: #ffb454;
}
.metric-teal {
    font-size: 16px;
    font-weight: 800;
    color: #5bd9c8;
}
.mono {
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 11px;
}
.mono-large {
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    font-size: 14px;
    font-weight: 700;
    color: #6fe2d0;
}
.muted {
    color: #849aab;
}
.faint {
    color: #60778a;
    font-size: 11px;
}
.accent {
    color: #ffb454;
}
.teal {
    color: #5bd9c8;
}
.danger {
    color: #ff7b7b;
}
.pill {
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 10px;
    font-weight: 800;
}
.pill-ok {
    background: #103d34;
    color: #77efce;
    border: 1px solid #1d6254;
}
.pill-run {
    background: #393017;
    color: #ffc56f;
    border: 1px solid #6d5a28;
}
.pill-fail {
    background: #421e22;
    color: #ff9b9b;
    border: 1px solid #71343b;
}
.pill-muted {
    background: #1b2935;
    color: #a5b8c7;
    border: 1px solid #30485b;
}
button {
    background: #182531;
    color: #e8f1f7;
    border: 1px solid #30475b;
    border-radius: 7px;
    padding: 7px 12px;
    box-shadow: none;
}
button:hover {
    background: #203344;
    border-color: #46647d;
}
button:active {
    background: #111c26;
}
button.suggested {
    background: #ffb454;
    color: #171108;
    border-color: #ffc274;
    font-weight: 800;
}
button.suggested:hover {
    background: #ffc16d;
}
button.ghost {
    background: transparent;
    border-color: #2f4658;
    color: #b9cbd7;
    padding: 5px 9px;
}
button.compact {
    padding: 4px 8px;
    font-size: 11px;
}
entry, spinbutton, dropdown {
    background: #0a1118;
    color: #e9f2f8;
    border: 1px solid #2a4052;
    border-radius: 7px;
    min-height: 30px;
}
entry:focus, spinbutton:focus, dropdown:focus {
    border-color: #d99a45;
}
spinbutton text {
    color: #e9f2f8;
}
dropdown button {
    background: transparent;
    border: none;
    padding: 5px 8px;
}
listbox {
    background: transparent;
}
listbox row {
    background: transparent;
    border-radius: 8px;
    padding: 8px;
}
listbox row:hover {
    background: #14212c;
}
listbox row:selected {
    background: #1b2d3c;
}
separator {
    background: #213342;
    min-height: 1px;
}
scrollbar slider {
    background: #31495c;
    border-radius: 4px;
    min-width: 6px;
    min-height: 32px;
}
"#;

pub fn install() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
