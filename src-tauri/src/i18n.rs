//! Strings used by the native side (tray menu, tooltip, notifications).
//! The popup has its own dictionary in ui/i18n.js.

pub struct T {
    pub open: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
    pub no_data: &'static str,
    pub charging: &'static str,
    pub watts: &'static str,
    pub low_title: &'static str,
    pub low_body: &'static str,
    pub full_title: &'static str,
    pub full_body: &'static str,
}

const UK: T = T {
    open: "Відкрити",
    settings: "Налаштування",
    quit: "Вийти",
    no_data: "немає даних",
    charging: "заряджається",
    watts: "Вт",
    low_title: "Низький заряд станції",
    low_body: "{name}: залишилося {soc}%",
    full_title: "Станцію заряджено",
    full_body: "{name}: 100%",
};

const RU: T = T {
    open: "Открыть",
    settings: "Настройки",
    quit: "Выйти",
    no_data: "нет данных",
    charging: "заряжается",
    watts: "Вт",
    low_title: "Низкий заряд станции",
    low_body: "{name}: осталось {soc}%",
    full_title: "Станция заряжена",
    full_body: "{name}: 100%",
};

const EN: T = T {
    open: "Open",
    settings: "Settings",
    quit: "Quit",
    no_data: "no data",
    charging: "charging",
    watts: "W",
    low_title: "Station battery low",
    low_body: "{name}: {soc}% left",
    full_title: "Station fully charged",
    full_body: "{name}: 100%",
};

pub fn get(lang: &str) -> &'static T {
    match lang {
        "uk" => &UK,
        "ru" => &RU,
        _ => &EN,
    }
}
