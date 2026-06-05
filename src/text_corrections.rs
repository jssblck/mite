//! Hardcoded literal OCR corrections for specific game text.
//!
//! These are last-resort fixups for known recognizer misreads in the target
//! games (missing inter-word spaces, confusable glyphs). They are pure content,
//! not OCR logic, so they live here rather than cluttering [`crate::ort_engine`].
//!
//! Order matters: replacements are applied top-to-bottom, so a longer phrase
//! must precede any shorter phrase that is a prefix of it (e.g. the three
//! `Odeto…` entries).

/// `(misread, correction)` pairs applied in order to recognized game text.
const COMMON_REPLACEMENTS: &[(&str, &str)] = &[
    (
        "Heliosisapproachingtargetaltitude",
        "Helios is approaching target altitude",
    ),
    (
        "Commandunresponsive.Controlprogramfailure",
        "Command unresponsive. Control program failure",
    ),
    ("Letourlight.shineinthe", "Let our light shine in the"),
    ("Youhaveanewmessage", "You have a new message"),
    (
        "Whatifwetransportitsomewhere",
        "What if we transport it somewhere",
    ),
    ("thatisn'taffectedby", "that isn't affected by"),
    ("Highaltitude", "High altitude"),
    (
        "Adjustthemeasurementlocation",
        "Adjust the measurement location",
    ),
    ("No,iftheimpactisthis", "No, if the impact is this"),
    (
        "widespread,anyadjustmentwouldbepointless",
        "widespread, any adjustment would be pointless",
    ),
    ("Modifythedetection", "Modify the detection"),
    (
        "No,eveniftheoreticallyfeasible",
        "No, even if theoretically feasible",
    ),
    ("there'snotimeto", "there's no time to"),
    ("constructthem", "construct them"),
    (
        "Thelaunchfailureiscausedbythe",
        "The launch failure is caused by the",
    ),
    (
        "launchfailureiscausedbythe",
        "launch failure is caused by the",
    ),
    (
        "Detector'sinabilitytomeasure",
        "Detector's inability to measure",
    ),
    ("Drive'soperatingparameters", "Drive's operating parameters"),
    ("That'sduetothehigh", "That's due to the high"),
    ("concentrationof", "concentration of"),
    (
        "Voidmatterinterferingwiththereadings",
        "Voidmatter interfering with the readings",
    ),
    (
        "Therearetwopotentialsolutions",
        "There are two potential solutions",
    ),
    ("Wecouldincreasethe", "We could increase the"),
    ("toleranceto", "tolerance to"),
    (
        "wecouldeliminatethesurrounding",
        "we could eliminate the surrounding",
    ),
    (
        "Butneitherofthoseoptionsarefeasibleinthetimewe",
        "But neither of those options are feasible in the time we",
    ),
    ("Hmm,that'snotrealistic", "Hmm, that's not realistic"),
    (
        "Theonlyplacesthataren'taffectedby",
        "The only places that aren't affected by",
    ),
    ("rightnoware", "right now are"),
    ("undertheprotectivebarrier", "under the protective barrier"),
    ("Orthe", "Or the"),
    (
        "Institutethousandsofmeters",
        "Institute thousands of meters",
    ),
    ("Whatabouthigh-powered", "What about high-powered"),
    ("Voidmatterneutralizers?", "Voidmatter neutralizers?"),
    ("withoutthe", "without the"),
    ("Drive'slight", "Drive's light"),
    (
        "allenergycomponentswillbeinoperable",
        "all energy components will be inoperable",
    ),
    ("Tellmewhat'shappening", "Tell me what's happening"),
    ("figureitouttogether", "figure it out together"),
    ("Atatimelikethis", "At a time like this"),
    ("O-oh?Sorry,lmjust", "O-oh? Sorry, I'm just"),
    ("PTaptoquit", "Tap to quit"),
    ("DTaptoquit", "Tap to quit"),
    ("Taptoquit", "Tap to quit"),
    ("MCheck Waves Line", "Check Waves Line"),
    ("VCheck Waves Line", "Check Waves Line"),
    ("Quest Re Wards", "Quest Rewards"),
    ("Re Wards", "Rewards"),
    ("Odetothe Second Sunrise", "Ode to the Second Sunrise"),
    ("Odetoth", "Ode to the"),
    ("Odeto", "Ode to"),
    ("Defeatthe Reactor Husk", "Defeat the Reactor Husk"),
    (
        "Reachthe Reactor Coreandsave",
        "Reach the Reactor Core and save",
    ),
    ("Headtotheinfirmary", "Head to the infirmary"),
    ("Headtotheinfirmar", "Head to the infirmary"),
    ("Talktothe", "Talk to the "),
    ("Discussthe", "Discuss the "),
    ("recentdevelopments", "recent developments"),
    (
        "Youshouldbemoreworriedabout",
        "You should be more worried about",
    ),
    ("She'llbefine", "She'll be fine"),
    ("Shouldwhat?", "Should what?"),
    (
        "Sothe Academygotitshandson",
        "So the Academy got its hands on",
    ),
    ("solidevidence?", "solid evidence?"),
    ("passingherclass.", "passing her class."),
    ("Thefinalverdictis", "The final verdict is"),
    ("Illstillhavetostayonthe", "I'll still have to stay on the"),
    ("watchlist\"forawhile", "watchlist\" for a while"),
    ("Dr.Herssensaidshewas", "Dr. Herssen said she was"),
    ("Overclockedfortoolong", "Overclocked for too long"),
    ("Imworried", "I'm worried"),
    ("Aphotoofyou", "A photo of you"),
    ("Lynaeunderthe New Sun", "Lynae under the New Sun"),
    ("Group Photo Underthe", "Group Photo Under the"),
    ("Tothe New World", "To the New World"),
    ("Ausk:", "Husk:"),
    ("Lv.1 oo", "Lv.100"),
    ("Lv.1 OO", "Lv.100"),
    ("Lv 10 o", "Lv.100"),
    ("V.90", "Lv.90"),
    ("v 80", "Lv.80"),
    ("Rover:Havot", "Rover:Havoc"),
    ("Rover: Havoo", "Rover:Havoc"),
    ("Rover:Havoo", "Rover:Havoc"),
    ("Mornyee", "Mornye"),
    ("Mornve", "Mornye"),
    ("lornye", "Mornye"),
    ("Lornye", "Mornye"),
];

/// Apply the [`COMMON_REPLACEMENTS`] table, in order, to recognized text.
pub fn apply_common_replacements(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in COMMON_REPLACEMENTS {
        out = out.replace(from, to);
    }
    out
}
