use gtk4::cairo;
use gtk4::glib::translate::{FromGlibPtrNone, ToGlibPtr};
use gtk4::pango;

#[link(name = "pangocairo-1.0")]
extern "C" {
    fn pango_cairo_show_layout(
        cr: *mut cairo::ffi::cairo_t,
        layout: *mut pango::ffi::PangoLayout,
    );
    fn pango_cairo_font_map_get_default() -> *mut pango::ffi::PangoFontMap;
}

pub fn show_layout(context: &cairo::Context, layout: &pango::Layout) {
    unsafe {
        pango_cairo_show_layout(context.to_raw_none(), layout.to_glib_none().0);
    }
}

pub fn context_with_default_font_map() -> pango::Context {
    let context = pango::Context::new();
    let font_map = unsafe {
        pango::FontMap::from_glib_none(pango_cairo_font_map_get_default())
    };
    context.set_font_map(Some(&font_map));
    context
}
