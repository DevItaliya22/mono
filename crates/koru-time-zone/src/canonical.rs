// SPDX-License-Identifier: MPL-2.0

//! Canonical targets for IANA Link names bundled by `chrono-tz` 0.10.4.
//!
//! `chrono-tz::Tz::name()` preserves the spelling parsed, including aliases,
//! so it cannot by itself satisfy Koru's storage contract. This table is
//! generated from every `Link TARGET LINK-NAME` record in that crate's pinned
//! tzdb source. `UTC` and `GMT` deliberately remain Koru's long-standing,
//! unambiguous canonical spellings.

pub(super) fn iana_link_target(name: &str) -> Option<&'static str> {
    Some(match name {
        "Africa/Accra" => "Africa/Abidjan",
        "Africa/Addis_Ababa" => "Africa/Nairobi",
        "Africa/Asmara" | "Africa/Asmera" => "Africa/Nairobi",
        "Africa/Bamako" | "Africa/Banjul" | "Africa/Conakry" | "Africa/Dakar"
        | "Africa/Freetown" | "Africa/Lome" | "Africa/Nouakchott" | "Africa/Ouagadougou"
        | "Africa/Timbuktu" => "Africa/Abidjan",
        "Africa/Bangui" | "Africa/Brazzaville" | "Africa/Douala" | "Africa/Kinshasa"
        | "Africa/Libreville" | "Africa/Luanda" | "Africa/Malabo" | "Africa/Niamey"
        | "Africa/Porto-Novo" => "Africa/Lagos",
        "Africa/Blantyre" | "Africa/Bujumbura" | "Africa/Gaborone" | "Africa/Harare"
        | "Africa/Kigali" | "Africa/Lubumbashi" | "Africa/Lusaka" => "Africa/Maputo",
        "Africa/Dar_es_Salaam"
        | "Africa/Djibouti"
        | "Africa/Kampala"
        | "Africa/Mogadishu"
        | "Indian/Antananarivo"
        | "Indian/Comoro"
        | "Indian/Mayotte" => "Africa/Nairobi",
        "Africa/Maseru" | "Africa/Mbabane" => "Africa/Johannesburg",
        "America/Anguilla"
        | "America/Antigua"
        | "America/Aruba"
        | "America/Blanc-Sablon"
        | "America/Curacao"
        | "America/Dominica"
        | "America/Grenada"
        | "America/Guadeloupe"
        | "America/Kralendijk"
        | "America/Lower_Princes"
        | "America/Marigot"
        | "America/Montserrat"
        | "America/Port_of_Spain"
        | "America/St_Barthelemy"
        | "America/St_Kitts"
        | "America/St_Lucia"
        | "America/St_Thomas"
        | "America/St_Vincent"
        | "America/Tortola"
        | "America/Virgin" => "America/Puerto_Rico",
        "America/Argentina/ComodRivadavia" | "America/Catamarca" => "America/Argentina/Catamarca",
        "America/Atikokan" | "America/Cayman" | "America/Coral_Harbour" => "America/Panama",
        "America/Atka" => "America/Adak",
        "America/Buenos_Aires" => "America/Argentina/Buenos_Aires",
        "America/Cordoba" | "America/Rosario" => "America/Argentina/Cordoba",
        "America/Creston" => "America/Phoenix",
        "America/Ensenada" | "America/Santa_Isabel" => "America/Tijuana",
        "America/Fort_Wayne" | "America/Indianapolis" => "America/Indiana/Indianapolis",
        "America/Godthab" => "America/Nuuk",
        "America/Jujuy" => "America/Argentina/Jujuy",
        "America/Knox_IN" => "America/Indiana/Knox",
        "America/Louisville" => "America/Kentucky/Louisville",
        "America/Mendoza" => "America/Argentina/Mendoza",
        "America/Montreal" | "America/Nassau" | "America/Nipigon" | "America/Thunder_Bay" => {
            "America/Toronto"
        }
        "America/Pangnirtung" => "America/Iqaluit",
        "America/Porto_Acre" => "America/Rio_Branco",
        "America/Rainy_River" => "America/Winnipeg",
        "America/Shiprock" => "America/Denver",
        "America/Yellowknife" => "America/Edmonton",
        "Antarctica/DumontDUrville" => "Pacific/Port_Moresby",
        "Antarctica/McMurdo" | "Antarctica/South_Pole" => "Pacific/Auckland",
        "Antarctica/Syowa" => "Asia/Riyadh",
        "Arctic/Longyearbyen" => "Europe/Berlin",
        "Asia/Aden" | "Asia/Kuwait" => "Asia/Riyadh",
        "Asia/Ashkhabad" => "Asia/Ashgabat",
        "Asia/Bahrain" => "Asia/Qatar",
        "Asia/Brunei" => "Asia/Kuching",
        "Asia/Calcutta" => "Asia/Kolkata",
        "Asia/Choibalsan" | "Asia/Ulan_Bator" => "Asia/Ulaanbaatar",
        "Asia/Chongqing" | "Asia/Chungking" | "Asia/Harbin" => "Asia/Shanghai",
        "Asia/Dacca" => "Asia/Dhaka",
        "Asia/Istanbul" => "Europe/Istanbul",
        "Asia/Kashgar" => "Asia/Urumqi",
        "Asia/Katmandu" => "Asia/Kathmandu",
        "Asia/Kuala_Lumpur" => "Asia/Singapore",
        "Asia/Macao" => "Asia/Macau",
        "Asia/Muscat" => "Asia/Dubai",
        "Asia/Phnom_Penh" | "Asia/Vientiane" => "Asia/Bangkok",
        "Asia/Rangoon" => "Asia/Yangon",
        "Asia/Saigon" => "Asia/Ho_Chi_Minh",
        "Asia/Tel_Aviv" => "Asia/Jerusalem",
        "Asia/Thimbu" => "Asia/Thimphu",
        "Asia/Ujung_Pandang" => "Asia/Makassar",
        "Atlantic/Faeroe" => "Atlantic/Faroe",
        "Atlantic/Jan_Mayen" => "Europe/Berlin",
        "Atlantic/Reykjavik" | "Atlantic/St_Helena" => "Africa/Abidjan",
        "Australia/ACT" | "Australia/Canberra" | "Australia/NSW" => "Australia/Sydney",
        "Australia/Currie" => "Australia/Hobart",
        "Australia/LHI" => "Australia/Lord_Howe",
        "Australia/North" => "Australia/Darwin",
        "Australia/Queensland" => "Australia/Brisbane",
        "Australia/South" => "Australia/Adelaide",
        "Australia/Tasmania" => "Australia/Hobart",
        "Australia/Victoria" => "Australia/Melbourne",
        "Australia/West" => "Australia/Perth",
        "Australia/Yancowinna" => "Australia/Broken_Hill",
        "Brazil/Acre" => "America/Rio_Branco",
        "Brazil/DeNoronha" => "America/Noronha",
        "Brazil/East" => "America/Sao_Paulo",
        "Brazil/West" => "America/Manaus",
        "Canada/Atlantic" => "America/Halifax",
        "Canada/Central" => "America/Winnipeg",
        "Canada/Eastern" => "America/Toronto",
        "Canada/Mountain" => "America/Edmonton",
        "Canada/Newfoundland" => "America/St_Johns",
        "Canada/Pacific" => "America/Vancouver",
        "Canada/Saskatchewan" => "America/Regina",
        "Canada/Yukon" => "America/Whitehorse",
        "CET" | "MET" => "Europe/Brussels",
        "Chile/Continental" => "America/Santiago",
        "Chile/EasterIsland" => "Pacific/Easter",
        "CST6CDT" => "America/Chicago",
        "Cuba" => "America/Havana",
        "EET" => "Europe/Athens",
        "Egypt" => "Africa/Cairo",
        "Eire" => "Europe/Dublin",
        "EST" => "America/Panama",
        "EST5EDT" => "America/New_York",
        "Etc/GMT+0" | "Etc/GMT0" | "Etc/GMT-0" | "Etc/Greenwich" => "Etc/GMT",
        "Etc/UCT" | "Etc/Universal" | "Etc/Zulu" => "Etc/UTC",
        "Europe/Amsterdam" | "Europe/Luxembourg" => "Europe/Brussels",
        "Europe/Belfast" | "Europe/Guernsey" | "Europe/Isle_of_Man" | "Europe/Jersey" => {
            "Europe/London"
        }
        "Europe/Bratislava" => "Europe/Prague",
        "Europe/Busingen" | "Europe/Vaduz" => "Europe/Zurich",
        "Europe/Copenhagen" | "Europe/Oslo" | "Europe/Stockholm" => "Europe/Berlin",
        "Europe/Kiev" | "Europe/Uzhgorod" | "Europe/Zaporozhye" => "Europe/Kyiv",
        "Europe/Ljubljana" | "Europe/Podgorica" | "Europe/Sarajevo" | "Europe/Skopje"
        | "Europe/Zagreb" => "Europe/Belgrade",
        "Europe/Mariehamn" => "Europe/Helsinki",
        "Europe/Monaco" => "Europe/Paris",
        "Europe/Nicosia" => "Asia/Nicosia",
        "Europe/San_Marino" | "Europe/Vatican" => "Europe/Rome",
        "Europe/Tiraspol" => "Europe/Chisinau",
        "GB" | "GB-Eire" => "Europe/London",
        "GMT+0" | "GMT0" | "GMT-0" | "Greenwich" => "Etc/GMT",
        "Hongkong" => "Asia/Hong_Kong",
        "HST" => "Pacific/Honolulu",
        "Iceland" => "Africa/Abidjan",
        "Indian/Christmas" => "Asia/Bangkok",
        "Indian/Cocos" => "Asia/Yangon",
        "Indian/Kerguelen" => "Indian/Maldives",
        "Indian/Mahe" | "Indian/Reunion" => "Asia/Dubai",
        "Iran" => "Asia/Tehran",
        "Israel" => "Asia/Jerusalem",
        "Jamaica" => "America/Jamaica",
        "Japan" => "Asia/Tokyo",
        "Kwajalein" => "Pacific/Kwajalein",
        "Libya" => "Africa/Tripoli",
        "Mexico/BajaNorte" => "America/Tijuana",
        "Mexico/BajaSur" => "America/Mazatlan",
        "Mexico/General" => "America/Mexico_City",
        "MST" => "America/Phoenix",
        "MST7MDT" | "Navajo" => "America/Denver",
        "NZ" => "Pacific/Auckland",
        "NZ-CHAT" => "Pacific/Chatham",
        "Pacific/Chuuk" | "Pacific/Truk" | "Pacific/Yap" => "Pacific/Port_Moresby",
        "Pacific/Enderbury" => "Pacific/Kanton",
        "Pacific/Funafuti" | "Pacific/Majuro" | "Pacific/Wake" | "Pacific/Wallis" => {
            "Pacific/Tarawa"
        }
        "Pacific/Johnston" => "Pacific/Honolulu",
        "Pacific/Midway" | "Pacific/Samoa" => "Pacific/Pago_Pago",
        "Pacific/Pohnpei" | "Pacific/Ponape" => "Pacific/Guadalcanal",
        "Pacific/Saipan" => "Pacific/Guam",
        "Poland" => "Europe/Warsaw",
        "Portugal" => "Europe/Lisbon",
        "PRC" => "Asia/Shanghai",
        "PST8PDT" => "America/Los_Angeles",
        "ROC" => "Asia/Taipei",
        "ROK" => "Asia/Seoul",
        "Singapore" => "Asia/Singapore",
        "Turkey" => "Europe/Istanbul",
        "UCT" | "Universal" | "Zulu" => "Etc/UTC",
        "US/Alaska" => "America/Anchorage",
        "US/Aleutian" => "America/Adak",
        "US/Arizona" => "America/Phoenix",
        "US/Central" => "America/Chicago",
        "US/Eastern" => "America/New_York",
        "US/East-Indiana" => "America/Indiana/Indianapolis",
        "US/Hawaii" => "Pacific/Honolulu",
        "US/Indiana-Starke" => "America/Indiana/Knox",
        "US/Michigan" => "America/Detroit",
        "US/Mountain" => "America/Denver",
        "US/Pacific" => "America/Los_Angeles",
        "US/Samoa" => "Pacific/Pago_Pago",
        "WET" => "Europe/Lisbon",
        "W-SU" => "Europe/Moscow",
        _ => return None,
    })
}
