//! Strings used by the native side (tray menu, tooltip, notifications).
//! The popup has its own dictionary in ui/i18n.js.

pub struct T {
    pub open: &'static str,
    pub settings: &'static str,
    pub settings_title: &'static str,
    pub quit: &'static str,
    pub no_data: &'static str,
    pub lost: &'static str,
    pub last_known: &'static str,
    pub charging: &'static str,
    pub watts: &'static str,
    pub volts: &'static str,
    pub grid: &'static str,
    pub low_title: &'static str,
    pub low_body: &'static str,
    pub full_title: &'static str,
    pub full_body: &'static str,
}

const UK: T = T {
    open: "Відкрити",
    settings: "Налаштування…",
    settings_title: "Station Battery – налаштування",
    quit: "Вийти",
    no_data: "немає даних",
    lost: "немає зв’язку",
    last_known: "останній відомий заряд",
    charging: "заряджається",
    watts: "Вт",
    volts: "В",
    grid: "Мережа",
    low_title: "Низький заряд станції",
    low_body: "{name}: залишилося {soc}%",
    full_title: "Станцію заряджено",
    full_body: "{name}: 100%",
};

const RU: T = T {
    open: "Открыть",
    settings: "Настройки…",
    settings_title: "Station Battery – настройки",
    quit: "Выйти",
    no_data: "нет данных",
    lost: "нет связи",
    last_known: "последний известный заряд",
    charging: "заряжается",
    watts: "Вт",
    volts: "В",
    grid: "Сеть",
    low_title: "Низкий заряд станции",
    low_body: "{name}: осталось {soc}%",
    full_title: "Станция заряжена",
    full_body: "{name}: 100%",
};

const EN: T = T {
    open: "Open",
    settings: "Settings…",
    settings_title: "Station Battery Settings",
    quit: "Quit",
    no_data: "no data",
    lost: "no connection",
    last_known: "last known level",
    charging: "charging",
    watts: "W",
    volts: "V",
    grid: "Grid",
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
