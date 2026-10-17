use crate::images;
use std::sync::LazyLock;

pub(crate) struct Skin {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) fruits: [&'static [u8]; 11],
}
macro_rules! fruits {
    ($skin:literal) => {
        [
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-1.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-2.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-3.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-4.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-5.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-6.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-7.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-8.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-9.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-10.bin")),
            include_bytes!(concat!("../assets/skins/", $skin, "/fruit-11.bin")),
        ]
    };
}
pub(crate) const SKINS: [Skin; 2] = [
    Skin {
        id: "classic",
        name: "经典水果",
        fruits: fruits!("classic"),
    },
    Skin {
        id: "qfruit",
        name: "Q版水果",
        fruits: fruits!("qfruit"),
    },
];
static PREVIEWS: LazyLock<[[String; 11]; 2]> = LazyLock::new(|| {
    SKINS.each_ref().map(|skin| {
        skin.fruits
            .map(|bytes| images::indexed_preview(bytes).expect("bundled skin image"))
    })
});
pub(crate) fn preview(skin: usize, level: usize) -> &'static str {
    &PREVIEWS[skin][level]
}
