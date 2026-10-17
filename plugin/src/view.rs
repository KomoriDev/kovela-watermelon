use crate::ui::{Element, ElementType, Event, FlexDirection};

pub(crate) fn column() -> Element {
    Element::new(ElementType::Div, None)
        .flex()
        .flex_direction(FlexDirection::Column)
        .gap(12)
        .width_full()
        .min_width(0)
}
pub(crate) fn row() -> Element {
    Element::new(ElementType::Div, None)
        .flex()
        .align_center()
        .gap(8)
        .width_full()
        .min_width(0)
}
pub(crate) fn text(value: &str) -> Element {
    Element::new(ElementType::P, Some(value))
        .size(14)
        .text_color("#e6e0e9")
}
pub(crate) fn label(value: &str) -> Element {
    text(value).size(12).text_color("#cac4d0")
}
pub(crate) fn title(value: &str) -> Element {
    text(value).size(22)
}
pub(crate) fn heading(value: &str) -> Element {
    text(value).size(16)
}
pub(crate) fn button(value: &str, id: &str, primary: bool, disabled: bool) -> Element {
    let button = Element::new(ElementType::Button, Some(value))
        .on(Event::Click, id)
        .height(40)
        .min_width(0)
        .flex_grow(1.0)
        .flex_shrink(1.0)
        .radius(20)
        .bg(if primary { "#b6dfa3" } else { "#2b2930" })
        .text_color(if primary { "#243b1c" } else { "#e6e0e9" });
    if disabled { button.disabled() } else { button }
}
pub(crate) fn panel() -> Element {
    column()
        .padding(12)
        .radius(12)
        .bg("#211f26")
        .border(1, "#49454f")
}
pub(crate) fn image(source: &str, size: u32) -> Element {
    Element::new(ElementType::Image, Some(source))
        .width(size)
        .height(size)
        .flex_shrink(0.0)
        .prop("alt", "水果图片")
}
pub(crate) fn tile(selected: bool) -> Element {
    Element::new(ElementType::Button, None)
        .flex()
        .flex_direction(FlexDirection::Column)
        .align_center()
        .justify_center()
        .gap(8)
        .min_width(0)
        .padding(8)
        .radius(12)
        .bg(if selected { "#354b2c" } else { "#2b2930" })
        .border(1, if selected { "#b6dfa3" } else { "#49454f" })
}

pub(crate) fn skin_tile(selected: bool) -> Element {
    tile(selected).height(128)
}

pub(crate) fn fruit_tile(selected: bool) -> Element {
    tile(selected).height(96)
}
pub(crate) fn progress(value: u32) -> Element {
    Element::new(ElementType::Progress, None)
        .prop("value", &value.to_string())
        .prop("max", "100")
        .width_full()
}
