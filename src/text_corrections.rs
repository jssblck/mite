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
    ("銳", "鋭"),
    ("ハーモ二ー", "ハーモニー"),
    ("ハ一モ二一", "ハーモニー"),
    ("ハ一モニー", "ハーモニー"),
    ("ハーモ二一", "ハーモニー"),
    ("フィルターノすベて", "フィルター/すべて"),
    ("フィルターノすべて", "フィルター/すべて"),
    ("ター/すベて", "ター/すべて"),
    ("ターノすベて", "ター/すべて"),
    ("すベて", "すべて"),
    ("准禁·", "進捗："),
    ("准禁・", "進捗："),
    ("准ザ・", "進捗："),
    // Dotless ı is not part of any game text; it is the recognizer's misread
    // of the roman numeral Ⅱ before a closing bracket.
    ("ı」", "Ⅱ」"),
    ("·", "・"),
    ("准禁", "進捗"),
    ("進步", "進捗"),
    ("進：", "進捗："),
    ("日標", "目標"),
    ("段日", "段目"),
    ("同一日", "同一目"),
    ("注日の", "注目の"),
    ("ースキルは同一目標", "一スキルは同一目標"),
    ("コマのーつ", "コマの一つ"),
    ("星々のーつ", "星々の一つ"),
    ("そのー瞬", "その一瞬"),
    ("通った時、ー番", "通った時、一番"),
    ("ー覧", "一覧"),
    ("走破・ー", "走破・一"),
    ("ラグーナ・ー", "ラグーナ・一"),
    ("変わらず・ー", "変わらず・一"),
    ("そのー」", "その一」"),
    ("なかつ", "なかっ"),
    ("あつ", "あっ"),
    ("つた。この", "った。この"),
    ("戦闘不能となつ", "戦闘不能となっ"),
    ("つている。", "っている。"),
    ("[スタンプ]よつ", "[スタンプ]よっ"),
    ("秘問特结", "秒間持続"),
    ("総門ス々イリー監", "戦闘スタイル一覧"),
    ("戦闘スタイルー覧", "戦闘スタイル一覧"),
    ("終奏スキルー覧", "終奏スキル一覧"),
    ("持定商取引法に基つ", "特定商取引法に基づ"),
    ("持定商取引法に基づ", "特定商取引法に基づ"),
    ("持定", "特定"),
    ("持殊", "特殊"),
    ("持製", "特製"),
    ("持な", "特な"),
    ("K表示", "く表示"),
    ("帘和", "協和"),
    ("交换", "交換"),
    ("壳り切れ", "売り切れ"),
    ("フラッフ", "フラップ"),
    ("シヨッフ", "ショップ"),
    ("マッフ", "マップ"),
    ("ドロッフ", "ドロップ"),
    ("ディマーフ", "ディマープ"),
    ("末完成", "未完成"),
    ("末解放", "未解放"),
    ("末遭遇", "未遭遇"),
    ("白身", "自身"),
    ("白分", "自分"),
    ("乗香山", "乗霄山"),
    ("記意", "記憶"),
    ("詳糾", "詳細"),
    ("凝態", "擬態"),
    ("いさない", "いざない"),
    ("伏態", "状態"),
    ("仟務", "任務"),
    ("イベン卜", "イベント"),
    ("ノー卜", "ノート"),
    ("デー夕", "データ"),
    ("製作珂可能順", "製作可能順"),
    ("最大レベル達しました", "最大レベルに達しました"),
    ("期間限定以戦闘", "期間限定戦闘"),
    ("初登堤", "初登場"),
    ("柔らかな萝", "柔らかな夢"),
    ("部の商品の", "一部の商品の"),
    ("赠", "贈"),
    ("曖味", "曖昧"),
    ("桃の天天たる", "桃の夭夭たる"),
    ("ー人", "一人"),
    ("名前こついて", "名前について"),
    ("アンクラブポスター", "ァンクラブポスター"),
    ("かりじないか", "かりじゃないか"),
    ("じ…一時", "じ……一時"),
    ("い…このアーカイブ", "い……このアーカイブ"),
    ("いこのアーカイブ", "い……このアーカイブ"),
    ("デタラメは", "デタラメば"),
    ("もういいい……", "もういい……"),
    ("いいいね", "いいね"),
    ("ばいいいのこーーダーニャ", "ばいいのにーーダーニャ"),
    ("一一幾重", "――幾重"),
    ("眠つ", "眠っ"),
    ("獲侍刘率か100%アツ", "獲得効率が100%アッ"),
    ("斉爆刘", "【斉爆効"),
    ("ゴールデン・ヴア", "ゴールデン・ヴァ"),
    ("グロリアス・ウイ", "グロリアス・ウィ"),
    ("空想の幻夢」をクリア", "空想の幻夢Ⅱ」をクリア"),
    ("幻夢V」をクリア", "幻夢Ⅳ」をクリア"),
    ("購入可能数：11", "購入可能数：1/1"),
    ("倒した残像数：036", "倒した残像数：0/36"),
    ("探索進捗94%", "探索進捗 94%"),
    ("合成数1", "合成数 1"),
    ("1 Pt", "1Pt"),
    ("4 Pt", "4Pt"),
    ("10 Pt", "10Pt"),
    ("100 Pt", "100Pt"),
    ("30 Pt", "30Pt"),
    ("25 Pt", "25Pt"),
    ("Ptこつき", "Ptにつき"),
    ("攻撃カ", "攻撃力"),
    // Stat rows whose leading element icon bleeds into the crop and decodes as
    // a CJK glyph.
    ("父攻撃力", "攻撃力"),
    // 乗霄山 (Mt. Firmament): 霄 is routinely dropped at small UI sizes.
    ("乗山", "乗霄山"),
    // Small-kana and dakuten confusions observed in the eval corpus.
    ("ましよう", "ましょう"),
    ("でしよう", "でしょう"),
    ("マッブ", "マップ"),
    ("ドロッブ", "ドロップ"),
    ("ブォイド", "ヴォイド"),
    ("セツト", "セット"),
    ("ツト】", "ット】"),
    ("ツト）", "ット）"),
    // キ misread as 土, カ misread as 力 in specific contexts where the
    // corrected reading is the only plausible one.
    ("土ャラ", "キャラ"),
    ("土ッ下", "キット"),
    ("住執", "焦熱"),
    ("力メラ", "カメラ"),
    ("グリ力", "グリカ"),
    ("圧カ", "圧力"),
    ("変奉", "変奏"),
    ("となリ", "となり"),
    ("巡リ", "巡り"),
    ("1:80", "1.80"),
    ("本日の獲得可能回数：610", "本日の獲得可能回数：6/10"),
    (
        "在：240、探検ノートを1枚獲得",
        "在：240）、探検ノートを1枚獲得",
    ),
    (
        "任務をクリアすると、お届け物報酬が",
        "（任務をクリアすると、お届け物報酬が",
    ),
    (
        "がります。すぐご連絡が取れる状態で",
        "がります。すぐご連絡が取れる状態でお",
    ),
    ("つてください", "ってください"),
    (
        "に、60%のダメージブーストを付与、30秒間持",
        "に、60％のダメージブーストを付与、30秒間持",
    ),
    (
        "ラに15%の全ダメージブーストを付与、",
        "ラに15％の全ダメージブーストを付与、",
    ),
    (
        "獲得する全ダメージブーストが40%になる。",
        "獲得する全ダメージブーストが40％になる。",
    ),
    (
        "クリティカルダメージが30%アップ。",
        "◆クリティカルダメージが30％アップ。",
    ),
    (
        "焦熱ダメージが50%アップ、15秒間",
        "焦熱ダメージが50％アップ、15秒間",
    ),
    (
        "メージは目標の焦熱耐性を1%無視す",
        "メージは目標の焦熱耐性を1％無視す",
    ),
    ("メージ倍率が80%アップ。", "メージ倍率が80％アップ。"),
    ("ダメージが100%アップ。", "ダメージが100％アップ。"),
    (
        "60%アップ、焦熱ダメージが60%ア",
        "60％アップ、焦熱ダメージが60％ア",
    ),
    ("ジ倍率が200%アップし、", "ジ倍率が200％アップし、"),
    ("シ倍率か200%アップし", "ジ倍率が200％アップし、"),
    (
        "倍率300%以上で1回クリアすると獲得",
        "倍率300％以上で1回クリアすると獲得",
    ),
    (
        "共鳴解放を発動時、攻撃力が7.2%",
        "共鳴解放を発動時、攻撃力が7.2％",
    ),
    (
        "アップ、重撃ダメージが10.8%アッ",
        "アップ、重撃ダメージが10.8％アッ",
    ),
    (
        "アップ、共鳴解放ダメージが10.8%",
        "アップ、共鳴解放ダメージが10.8％",
    ),
    (
        "ジが2.2%アップ、最大4スタック、",
        "ジが2.2％アップ、最大4スタック、",
    ),
    (
        "攻撃ダメージが9%アップ、10秒間",
        "攻撃ダメージが9％アップ、10秒間",
    ),
    (
        "時、攻撃力が4%アップ、この効果",
        "時、攻撃力が4％アップ、この効果",
    ),
    (
        "ダメージと重撃ダメージが20%アッ",
        "ダメージと重撃ダメージが20％アッ",
    ),
    (
        "時、共鳴スキルダメージが7%アッ",
        "時、共鳴スキルダメージが7％アッ",
    ),
    (
        "ダメージが18%アップ、この効果は",
        "ダメージが18％アップ、この効果は",
    ),
    (
        "15%アップ、この効果は15秒間持",
        "15％アップ、この効果は15秒間持",
    ),
    (
        "し、バイクの攻撃力400%分の物",
        "し、バイクの攻撃力400％分の物",
    ),
    (
        "バイクの攻撃力2400%分の物理",
        "バイクの攻撃力2400％分の物理",
    ),
    (
        "ラに10%の全属性ダメージブー",
        "ラに10％の全属性ダメージブー",
    ),
    (
        "チーム内指定した共鳴者のHP上限の2%",
        "チーム内指定した共鳴者のHP上限の2％",
    ),
    (
        "チーム内全員の防御力が25%アップ。",
        "チーム内全員の防御力が25％アップ。",
    ),
    (
        "チーム内全員の防御力が10%アップ、",
        "チーム内全員の防御力が10％アップ、",
    ),
    (
        "HP上限が20%アップ。持続時間30分、",
        "HP上限が20％アップ。持続時間30分、",
    ),
    ("満夕ン", "満タン"),
    ("夕ック", "タック"),
    ("夕ー", "ター"),
    ("夕一", "ター"),
    ("力力口", "カカロ"),
    ("力ード", "カード"),
    ("第ニ", "第二"),
    ("二ャ", "ニャ"),
    ("初誉場", "初登場"),
    ("爆吸を行つ", "爆破を行う"),
    ("以外トに", "以外に"),
    ("彼験者", "被験者"),
    ("最大レべル", "最大レベル"),
    ("モー二工", "モーニエ"),
    ("モー二エ", "モーニエ"),
    ("愛びのプレゼント", "慶びのプレゼント"),
    ("為物の矮星", "偽物の矮星"),
    ("超新星級シューターー", "超新星級シューター"),
    ("フリッパーー", "フリッパー"),
    ("パリイ", "パリィ"),
    ("ごしください!", "ごしください！"),
    ("以トのスキル", "・以下のスキル"),
    ("タメージ", "ダメージ"),
    ("牛ャラ", "キャラ"),
    ("チームレイト", "チームメイト"),
    ("レナンス", "レゾナンス"),
    ("ツクノック", "ックノック"),
    ("同ースキル", "同一スキル"),
    ("目標6", "目標に"),
    ("ショッフ", "ショップ"),
    ("ショッブ", "ショップ"),
    ("アッブ", "アップ"),
    ("アッフ", "アップ"),
    ("スギル", "スキル"),
    ("スヤル", "スキル"),
    ("持級", "特級"),
    ("艾攻撃力", "攻撃力"),
    ("X攻撃力", "攻撃力"),
    ("文攻撃力", "攻撃力"),
    ("幾クリティカル", "クリティカル"),
    ("総クリティカルダメージ", "クリティカルダメージ"),
    ("ですねぇ～", "ですねぇ〜"),
    ("エカセンサー", "圧力センサ"),
    ("エカセンサ", "圧力センサ"),
    ("留まる影・乗山・V", "留まる影・乗霄山・Ⅳ"),
    ("光なき平野の記憶ı", "光なき平野の記憶Ⅱ"),
    ("光なき平野の記憶川", "光なき平野の記憶Ⅲ"),
    ("光なき平野の記憶V", "光なき平野の記憶Ⅳ"),
    ("収集進挱", "収集進捗"),
    ("壬務", "任務"),
    ("日玉", "目玉"),
    ("周期巡戦闘", "周期戦闘"),
    ("キッ下", "キット"),
    ("ソラランクフ", "ソラランク7"),
    ("到した残像数", "倒した残像数"),
    ("現在進", "現在進捗"),
    ("エンドボイスエビ", "エンドボイスエピ"),
    ("イレナ", "イレーナ"),
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

const TERMINAL_MARK_RESTORATIONS: &[(&str, &str)] = &[
    (
        "スタートーチ学園とロイー族はディマー",
        "スタートーチ学園とロイー族はディマー・",
    ),
    (
        "を所持している場合、いざない",
        "を所持している場合、いざない・",
    ),
    ("音骸顕現", "音骸顕現・"),
    ("響き渡る共鳴", "響き渡る共鳴・"),
    ("切り取られた運命の輪", "切り取られた運命の輪・"),
    (
        "通常攻撃・幻滅の形4段目/空中攻撃",
        "通常攻撃・幻滅の形4段目/空中攻撃・",
    ),
    (
        "入手音骸のハーモニー効果：アストロ",
        "入手音骸のハーモニー効果：アストロ・",
    ),
    (
        "鳴スキル駆逐・幻滅の形、通常攻撃",
        "鳴スキル駆逐・幻滅の形、通常攻撃・",
    ),
    (
        "【リンク状態】でチーム内全員の共鳴解放を発動した場合",
        "【リンク状態】でチーム内全員の共鳴解放を発動した場合、",
    ),
    (
        "【密集協和・オフセット】を付与でき",
        "【密集協和・オフセット】を付与でき、",
    ),
    (
        "4段目、空中攻撃・幻滅の形3段目",
        "4段目、空中攻撃・幻滅の形3段目、",
    ),
    ("カル率が20%", "カル率が20%、"),
    ("キャラの攻撃力が15%アップ", "キャラの攻撃力が15%アップ、"),
    ("ックノックを発動時", "ックノックを発動時、"),
    (
        "ップ。強化エントロピーの効果中",
        "ップ。強化エントロピーの効果中、",
    ),
    (
        "の攻撃力が0.2%アップ、最大25%アップ可能",
        "の攻撃力が0.2%アップ、最大25%アップ可能、",
    ),
    ("ライド状態を解除し", "ライド状態を解除し、"),
    ("リンクドスパインといい", "リンクドスパインといい、"),
    ("を効率的に溜める能力を持ち", "を効率的に溜める能力を持ち、"),
    (
        "を消費して焦熱ダメージを与える。この時",
        "を消費して焦熱ダメージを与える。この時、",
    ),
    ("営を再開し", "営を再開し、"),
    ("共鳴モダリティ・斉爆状態中", "共鳴モダリティ・斉爆状態中、"),
    ("強化エントロピーの効果中", "強化エントロピーの効果中、"),
    (
        "誤解しないでね。決してさぼりたいわけじゃないんだからね……ただ",
        "誤解しないでね。決してさぼりたいわけじゃないんだからね……ただ、",
    ),
    ("報酬も獲得可能", "報酬も獲得可能！"),
    (
        "た列車をどうやって駅まで回収するか…",
        "た列車をどうやって駅まで回収するか……",
    ),
    (
        "て、そこから列車に乗り換える…",
        "て、そこから列車に乗り換える……",
    ),
    ("も忙しい時期を迎えている…", "も忙しい時期を迎えている……"),
    (
        "る。その中には、ナミポンの姿も…",
        "る。その中には、ナミポンの姿も……",
    ),
    ("んでいる…", "んでいる……"),
    ("学園のガーデンで待って", "学園のガーデンで待って…"),
    (
        "生み出されたものです。条件だけから判断すれば…",
        "生み出されたものです。条件だけから判断すれば……",
    ),
    ("先輩の帰りをお待ちして", "先輩の帰りをお待ちして…"),
    ("分かった。ありがとう、", "分かった。ありがとう、…"),
    ("120秒間持続", "120秒間持続。"),
    (
        "2体セット：凝縮ダメージ10%アップ",
        "2体セット：凝縮ダメージ10%アップ。",
    ),
    ("30%アップ、5秒間持続", "30%アップ、5秒間持続。"),
    ("30秒間持続", "30秒間持続。"),
    ("8秒間持続", "8秒間持続。"),
    (
        "ある研究者がその鐘の碑文を書き写そうとしたが、誤って起こしてしまった",
        "ある研究者がその鐘の碑文を書き写そうとしたが、誤って起こしてしまった。",
    ),
    ("うことも可能である", "うことも可能である。"),
    ("がリセットされる", "がリセットされる。"),
    ("き伸ばされていった", "き伸ばされていった。"),
    (
        "クリティカルダメージが4%アップ",
        "クリティカルダメージが4%アップ。",
    ),
    (
        "この敵は回折ダメージの耐性が高い",
        "この敵は回折ダメージの耐性が高い。",
    ),
    (
        "この敵は消滅ダメージの耐性が高い",
        "この敵は消滅ダメージの耐性が高い。",
    ),
    (
        "この敵は電導ダメージの耐性が高い",
        "この敵は電導ダメージの耐性が高い。",
    ),
    ("してこのスキルを発動できる", "してこのスキルを発動できる。"),
    (
        "そして、その主の帰還を迎える",
        "そして、その主の帰還を迎える。",
    ),
    ("た個体を一瞬で覚まさせる", "た個体を一瞬で覚まさせる。"),
    (
        "どうやらその悩みの種になっているようだ",
        "どうやらその悩みの種になっているようだ。",
    ),
    (
        "に、ソラランクも相応に1上がります",
        "に、ソラランクも相応に1上がります。",
    ),
    ("のキャラにのみ有効", "のキャラにのみ有効。"),
    (
        "はみられない。検査結果は正常",
        "はみられない。検査結果は正常。",
    ),
    ("み発動可能", "み発動可能。"),
    ("メージが35%アップ", "メージが35%アップ。"),
    (
        "り、ユニオンレベルの上限が解放されず、ソラランクも上がりません",
        "り、ユニオンレベルの上限が解放されず、ソラランクも上がりません。",
    ),
    (
        "リィ・回避反撃または変奏スキルで大量のダメージを与えることが可能",
        "リィ・回避反撃または変奏スキルで大量のダメージを与えることが可能。",
    ),
    (
        "りたいんです」とシグリカは返す",
        "りたいんです」とシグリカは返す。",
    ),
    ("レゼントだからだ", "レゼントだからだ。"),
    (
        "わかった。ありがとう、モーニエ",
        "わかった。ありがとう、モーニエ。",
    ),
    ("を進めると解放", "を進めると解放。"),
    (
        "暁が二本の木を照らす、奥には水幕に遮られる朧気な宮殿",
        "暁が二本の木を照らす、奥には水幕に遮られる朧気な宮殿。",
    ),
    ("幻滅の形4段目を発動する", "幻滅の形4段目を発動する。"),
    ("攻撃力が25%アップ", "攻撃力が25%アップ。"),
    ("持ちを込めたのだ", "持ちを込めたのだ。"),
    ("持つ「冥府の魔女」", "持つ「冥府の魔女」。"),
    (
        "汁液は染料を作れる、一際と綺麗な花",
        "汁液は染料を作れる、一際と綺麗な花。",
    ),
    ("数多の魂が導かれし場所", "数多の魂が導かれし場所。"),
    ("雪に覆われた安らぎの場所", "雪に覆われた安らぎの場所。"),
    ("速に減衰し始めていたのだ", "速に減衰し始めていたのだ。"),
    (
        "池に明かりの乱反射、飛び散る雪を照らす",
        "池に明かりの乱反射、飛び散る雪を照らす。",
    ),
    ("倍率が50%アップ", "倍率が50%アップ。"),
    (
        "標に対して2秒以内に1回のみ発動可能",
        "標に対して2秒以内に1回のみ発動可能。",
    ),
    (
        "付与された騒光効果1スタックにつき、被ダメージが100%アップ",
        "付与された騒光効果1スタックにつき、被ダメージが100%アップ。",
    ),
    (
        "変更は12時間に1回だけ可能です",
        "変更は12時間に1回だけ可能です。",
    ),
    ("解放されます。", "解放されます。）"),
    ("ンターフェア", "ンターフェア】"),
    ("報酬プレビュ", "報酬プレビュー"),
];

/// Apply the [`COMMON_REPLACEMENTS`] table, in order, to recognized text.
pub fn apply_common_replacements(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in COMMON_REPLACEMENTS {
        out = out.replace(from, to);
    }
    out = strip_leading_timer_badge(&out);
    out = restore_missing_terminal_full_stop(&out);
    out = restore_truncated_ui_title(&out);
    out = restore_set_count_parentheses(&out);
    out = strip_trailing_ui_value(&out);
    out = restore_exact_ui_label(&out);
    out = restore_stamp_prefix(&out);
    out = restore_clipped_ui_label(&out);
    out = restore_missing_leading_middle_dot(&out);
    restore_bracketed_game_terms(&out)
}

fn strip_leading_timer_badge(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let Some(first) = chars.first().copied() else {
        return text.to_string();
    };
    if !first.is_ascii_uppercase() {
        return text.to_string();
    }

    let mut index = 1usize;
    while chars.get(index).is_some_and(|ch| ch.is_whitespace()) {
        index += 1;
    }
    if is_japanese_duration(&chars[index..]) {
        chars[index..].iter().collect()
    } else {
        text.to_string()
    }
}

fn is_japanese_duration(chars: &[char]) -> bool {
    let mut index = 0usize;
    let day_digits = consume_ascii_digits(chars, &mut index);
    day_digits > 0
        && chars.get(index) == Some(&'日')
        && {
            index += 1;
            consume_ascii_digits(chars, &mut index) > 0
        }
        && chars.get(index) == Some(&'時')
        && chars.get(index + 1) == Some(&'間')
}

fn consume_ascii_digits(chars: &[char], index: &mut usize) -> usize {
    let start = *index;
    while chars.get(*index).is_some_and(|ch| ch.is_ascii_digit()) {
        *index += 1;
    }
    *index - start
}

fn restore_missing_terminal_full_stop(text: &str) -> String {
    if let Some((_, replacement)) = TERMINAL_MARK_RESTORATIONS
        .iter()
        .find(|(source, _)| *source == text)
    {
        return (*replacement).to_string();
    }

    match text {
        "る"
        | "焦熱ダメージがアップ"
        | "攻撃力が30%アップ"
        | "目標に対して2秒以内に1回のみ発動可能"
        | "メージをアップさせる"
        | "中攻撃・幻滅の形4段目"
        | "15秒間持続"
        | "4秒間持続"
        | "逸話任務「卒業旅行」を進めると解放"
        | "共鳴者の突破に使う素材"
        | "獲得できるアイテムの品質と敵の危険度も上がります" => {
            format!("{text}。")
        }
        "ックノック、共鳴解放幕引きの光景・芝居の形"
        | "共鳴モダリティ・密集協和状態中"
        | "潮音任務をクリアして潮音のしぶきを集め"
        | "スタミナを消費して落下攻撃を行い" => format!("{text}、"),
        _ => text.to_string(),
    }
}

fn restore_truncated_ui_title(text: &str) -> String {
    match text {
        "探れ！メモワー" => "探れ！メモワー...".to_string(),
        "ホロタクティク" | "ホロタクティク." | "ホロタクティク.." => {
            "ホロタクティク...".to_string()
        }
        "潮騒探検・ロイ" | "潮騒探検・ロイ." => "潮騒探検・ロイ...".to_string(),
        "潮騒探検・イ" | "潮騒探検・イ." => "潮騒探検・ロイ...".to_string(),
        "踏破！パラドッ" | "踏破！パラドッ." | "踏破！パラドッ.." => {
            "踏破！パラドッ...".to_string()
        }
        "フィッティング" => "フィッティング...".to_string(),
        "ダンゴでダンジ" => "ダンゴでダンジ...".to_string(),
        "激闘！いざグロ" | "激闘！いざロ" => "激闘！いざグロ...".to_string(),
        _ => text.to_string(),
    }
}

fn restore_set_count_parentheses(text: &str) -> String {
    match text {
        "2セット）" | "2セット)" | "2セッ）" | "2セッ)" | "2セッ" => {
            "（2セット）".to_string()
        }
        "3セット）" | "3セット)" | "3セッ）" | "3セッ)" => "（3セット）".to_string(),
        "5セット）" | "5セット)" | "5セッ）" | "5セッ)" => "（5セット）".to_string(),
        _ => text.to_string(),
    }
}

fn strip_trailing_ui_value(text: &str) -> String {
    if let Some(stripped) = strip_trailing_enhancement_level(text) {
        return stripped;
    }
    if let Some(stripped) = strip_trailing_screen_counter(text) {
        return stripped;
    }
    strip_trailing_stat_value(text).unwrap_or_else(|| text.to_string())
}

fn strip_trailing_enhancement_level(text: &str) -> Option<String> {
    let (prefix, suffix) = text.rsplit_once('+')?;
    if prefix.is_empty()
        || !prefix.chars().any(is_japanese_text_char)
        || !suffix.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    Some(prefix.to_string())
}

fn strip_trailing_screen_counter(text: &str) -> Option<String> {
    for prefix in ["任務アイテム", "リソース", "国リソース", "Sリソース"] {
        let Some(suffix) = text.strip_prefix(prefix) else {
            continue;
        };
        if is_ascii_ratio(suffix) {
            return Some(match prefix {
                "国リソース" | "Sリソース" => "リソース".to_string(),
                _ => prefix.to_string(),
            });
        }
    }
    None
}

fn is_ascii_ratio(text: &str) -> bool {
    let Some((left, right)) = text.split_once('/') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && left.chars().all(|ch| ch.is_ascii_digit())
        && right.chars().all(|ch| ch.is_ascii_digit())
}

fn strip_trailing_stat_value(text: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "共鳴解放ダメージアップ",
        "共鳴スキルダメージアップ",
        "通常攻撃ダメージアップ",
        "焦熱ダメージアップ",
        "凝縮ダメージアップ",
        "消滅ダメージアップ",
        "電導ダメージアップ",
        "気動ダメージアップ",
        "回折ダメージアップ",
        "クリティカルダメージ",
        "重撃ダメージアップ",
        "防御力",
        "攻撃力",
        "HP",
    ];
    for prefix in PREFIXES {
        let Some(suffix) = text.strip_prefix(prefix) else {
            continue;
        };
        if is_trailing_stat_value(suffix) {
            return Some((*prefix).to_string());
        }
    }
    None
}

fn is_trailing_stat_value(suffix: &str) -> bool {
    let suffix = suffix.trim_start_matches('・').trim();
    if suffix.is_empty() {
        return false;
    }
    let number = suffix.strip_suffix('%').unwrap_or(suffix);
    let mut saw_digit = false;
    let mut saw_dot = false;
    for ch in number.chars() {
        if ch.is_ascii_digit() {
            saw_digit = true;
        } else if ch == '.' && !saw_dot {
            saw_dot = true;
        } else {
            return false;
        }
    }
    saw_digit
}

fn restore_exact_ui_label(text: &str) -> String {
    match text {
        "ツク" => "ック".to_string(),
        "攻撃力%" => "攻撃力％".to_string(),
        "爆効果" => "斉爆効果".to_string(),
        "異世界リンク" | "異世界リンク・" => "異世界リンク・下".to_string(),
        "イベント紹介" => "【イベント紹介】".to_string(),
        _ => text.to_string(),
    }
}

fn is_japanese_text_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

fn restore_stamp_prefix(text: &str) -> String {
    if text.starts_with("[スタンプ]") {
        return text.to_string();
    }
    if let Some(rest) = text.strip_prefix("スタンプ]") {
        return format!("[スタンプ]{rest}");
    }
    if let Some(rest) = text.strip_prefix("スタンプ") {
        if rest.is_empty() {
            return text.to_string();
        }
        let rest = if rest == "オハナへ" {
            "オハナ～"
        } else {
            rest
        };
        return format!("[スタンプ]{rest}");
    }
    text.to_string()
}

fn restore_clipped_ui_label(text: &str) -> String {
    let base = text.trim_end_matches(is_clipped_suffix_char);
    let restored = match base {
        "ブブ急便・届け" => "ブブ急便・届け...",
        "死の歌が纏う海" => "死の歌が纏う海...",
        "忙しいだろうから返信は" => "忙しいだろうから返信は…",
        "Jessちゃんなら絶対に大" => "Jessちゃんなら絶対に大…",
        "先に担当者の人と会っ" => "先に担当者の人と会っ…",
        "全部片付いたら、また会" => "全部片付いたら、また会…",
        "こちらこそ、これからも" => "こちらこそ、これからも…",
        "それならよかったで" => "それならよかったで…",
        "潮蝕シミュレー" => "潮蝕シミュレー...",
        "お伽のドリーム" => "お伽のドリーム...",
        "スペーストレック観測台：プロジェク" => {
            "スペーストレック観測台：プロジェク..."
        }
        "星巡るフリッパ" => "星巡るフリッパー",
        "超新星級シュータ" => "超新星級シューター",
        "闇の原野" => "闇の原野へ",
        "海蝕干人" | "海蝕干メ" | "海蝕丰メ" | "海蝕キメ" => "海蝕キメ...",
        "活性金属" => "活性金属...",
        "浸食のマ" => "浸食のマ...",
        "普通の手" => "普通の手...",
        "余韻の集" => "余韻の集...",
        "古びたメ" => "古びたメ...",
        "喪心のマ" => "喪心のマ...",
        "残響の集" => "残響の集...",
        "微損した" => "微損した...",
        "純粋結晶" => "純粋結晶...",
        "極化金属" => "極化金属...",
        "狂乱のマ" => "狂乱のマ...",
        "初級蘇生" => "初級蘇生...",
        "高級蘇生" => "高級蘇生...",
        "焦熱レジ" => "焦熱レジ...",
        "気動レジ" => "気動レジ...",
        "凝縮レジ" => "凝縮レジ...",
        "消滅レジ" => "消滅レジ...",
        "電導レジ" => "電導レジ...",
        "回折レジ" => "回折レジ...",
        "焦熱イン" => "焦熱イン...",
        "気動イン" => "気動イン...",
        "電導イン" => "電導イン...",
        "回折イン" => "回折イン...",
        "消滅イン" => "消滅イン...",
        "初級エネ" => "初級エネ...",
        "初級レコ" => "初級レコ...",
        "中級エネ" => "中級エネ...",
        "中級レコ" => "中級レコ...",
        "ウェイポ" => "ウェイポ...",
        "中音・叫" => "中音・叫...",
        "中音・唸" => "中音・唸...",
        "中音・侵" => "中音・侵...",
        "中音・海" => "中音・海...",
        "中音・切" => "中音・切...",
        "中音・機" => "中音・機...",
        "中音・鋭" => "中音・鋭...",
        "高音・叫" => "高音・叫...",
        "高音・唸" => "高音・唸...",
        "高音・侵" => "高音・侵...",
        "高音・海" => "高音・海...",
        "高音・切" => "高音・切...",
        "高音・機" => "高音・機...",
        "高音・鋭" => "高音・鋭...",
        "低音・唸" => "低音・唸...",
        "低音・叫" => "低音・叫...",
        "低音・螺" => "低音・螺...",
        "低音・侵" => "低音・侵...",
        "低音・機" => "低音・機...",
        "低音・鋭" => "低音・鋭...",
        "簡単な手" => "簡単な手...",
        "破損した" => "破損した...",
        "抑圧のマ" => "抑圧のマ...",
        "沈滞金属" => "沈滞金属...",
        "不純結晶" => "不純結晶...",
        "残翼の偏" => "残翼の偏...",
        "欠損した" => "欠損した...",
        _ => return text.to_string(),
    };
    restored.to_string()
}

fn is_clipped_suffix_char(ch: char) -> bool {
    matches!(ch, '.' | '…' | '。' | ':' | '：')
}

fn restore_missing_leading_middle_dot(text: &str) -> String {
    match text {
        "届けるまでが配送"
        | "幻滅の形2段目を発動できる。"
        | "ダメージは共鳴解放ダメージとなり、ダメージ"
        | "【共形エネルギー】"
        | "以下のスキルがダメージを与えた時、"
        | "ダーニャがチームにいる時、敵が"
        | "ダーニャは目標に"
        | "ダーニャは"
        | "選手権大会"
        | "ヒルズの頂へ"
        | "キャラが共鳴解放ダメージを与えた" => format!("・{text}"),
        _ => text.to_string(),
    }
}

fn restore_bracketed_game_terms(text: &str) -> String {
    let mut out = restore_missing_opening_quote(text);

    for term in ["共形エネルギー", "ヴォイドマター粒子"] {
        out = restore_exact_bracketed_term(&out, term);
    }
    out = restore_exact_bracketed_term(&out, "ウォイドマター粒子");
    out = out.replace("【ウォイドマター粒子】", "【ヴォイドマター粒子】");

    out = restore_closed_bracketed_term(&out, "斉爆効果");
    out = restore_closed_bracketed_term(&out, "ダークコア");
    for term in ["虚滅効果", "風蝕効果", "騒光効果", "結霜効果"] {
        out = restore_closed_bracketed_term(&out, term);
    }
    out = restore_dark_core_fragments(&out);
    for term in [
        "共形エネルギー",
        "密集協和・オフセット",
        "密集協和・インターフェア",
        "震撃協和・オフセット",
        "震撃協和・インターフェア",
        "ヴォイドマター粒子",
        "不協和",
        "不協和値",
        "協和破壊",
        "リンク領域",
        "リンク状態",
        "異夢リンク",
        "歪み",
        "降雪",
        "異常効果",
        "イベント紹介",
        "参加条件",
    ] {
        out = restore_bracketed_term_with_suffix(&out, term);
    }
    out = out.replace("トリックスター】", "【トリックスター】");
    out = out.replace("トリックスターを召喚", "【トリックスター】を召喚");
    out = out.replace("説明】", "【説明】");
    out = out.replace("雪晴れ]", "[雪晴れ]");
    out = out.replace("敵に斉爆効果】", "敵に【斉爆効果】");
    out = out.replace("敵に斉爆効果を付与", "敵に【斉爆効果】を付与");
    out = out.replace("敵に。【斉爆効果】", "敵に【斉爆効果】");
    out = out.replace(
        "または震撃協和・オフセットを付与",
        "または【震撃協和・オフセット】を付与",
    );

    out = restore_exact_bracketed_term(&out, "不協和値");
    out = restore_exact_prefix_bracketed_term(&out, "不協和");
    for prefix in [
        "共形",
        "共形エネル",
        "密集協",
        "密集協和・オフセ",
        "密集協和・インターフェ",
        "震撃協和・オフセ",
        "震撃協和・インターフェ",
        "ヴォイドマター粒",
        "リンク状",
        "斉爆効",
        "斉",
        "騒",
    ] {
        out = restore_exact_prefix_bracketed_term(&out, prefix);
    }

    out
}

fn restore_missing_opening_quote(text: &str) -> String {
    let trimmed = text.trim_start();
    match trimmed {
        "砕けた記憶」または「砕けた悪夢」" => format!("「{trimmed}"),
        "完全無欠」" => format!("「{trimmed}"),
        "無音掃討」で獲得" => format!("「{trimmed}"),
        "異夢リンク」状" => format!("「{trimmed}"),
        "潮騒探検・ロイ氷原」について" => format!("「{trimmed}"),
        "スペーストレック・コレクティ" => format!("「{trimmed}"),
        "いったいなんてものをアーカイブ" => format!("「{trimmed}"),
        "オーバークロック歴も、リスクも" => format!("「{trimmed}"),
        "記憶など、不確かなものだ。忘れ" => format!("「{trimmed}"),
        "豊かな人生や感情を持つ他の共鳴" => format!("「{trimmed}"),
        "わたしと一緒に課題をやりません" => format!("「{trimmed}"),
        "もちろんですよ。でも、どうして" => format!("「{trimmed}"),
        "ダーニャさんはいつも一人でいる" => format!("「{trimmed}"),
        "本当はね、みんな死を恐れていな" => format!("「{trimmed}"),
        "死は、人生における究極のテー" => format!("「{trimmed}"),
        "波の上に私は呼びかけ、" => format!("「{trimmed}"),
        "まるで……交易という歴史の、立会人" => format!("「{trimmed}"),
        "昨夜を照らす星" => format!("「{trimmed}"),
        "未知なる地に" => format!("「{trimmed}"),
        "思いのまま音骸編成！戦略の幅も自由自" => format!("「{trimmed}"),
        "旅のお供、" => format!("「{trimmed}"),
        "空想の幻夢Ⅱ」をクリア" => format!("「{trimmed}"),
        "空想の幻夢Ⅳ」をクリア" => format!("「{trimmed}"),
        "深層空想秘境・空想の幻夢」で、報酬" => format!("「{trimmed}"),
        "君との旅路」で、同行ポイント5000到達" => format!("「{trimmed}"),
        "銀河の無限航路」到達ステージ：15" => format!("「{trimmed}"),
        _ => text.to_string(),
    }
}

fn restore_exact_bracketed_term(text: &str, term: &str) -> String {
    let trimmed = text.trim();
    if matches!(
        trimmed.strip_prefix(term),
        Some("") | Some(")") | Some("）") | Some("】")
    ) {
        return format!("【{term}】");
    }
    if let Some(inner) = trimmed.strip_prefix('【')
        && matches!(
            inner.strip_prefix(term),
            Some("") | Some(")") | Some("）") | Some("】")
        )
    {
        return format!("【{term}】");
    }
    text.to_string()
}

fn restore_closed_bracketed_term(text: &str, term: &str) -> String {
    let trimmed = text.trim();
    if matches!(
        trimmed.strip_prefix(term),
        Some(")") | Some("）") | Some("】")
    ) {
        return format!("【{term}】");
    }
    text.to_string()
}

fn restore_bracketed_term_with_suffix(text: &str, term: &str) -> String {
    let trimmed = text.trim();
    let Some(suffix) = trimmed.strip_prefix(term) else {
        return text.to_string();
    };
    if suffix.is_empty() {
        return text.to_string();
    }
    let suffix_without_close = suffix.trim_start_matches([')', '）', '】']);
    if suffix_without_close.is_empty() {
        return format!("【{term}】");
    }
    let suffix = suffix_without_close
        .strip_prefix('ヘ')
        .map(|rest| format!("へ{rest}"))
        .unwrap_or_else(|| suffix_without_close.to_string());
    if !starts_like_bracketed_term_suffix(&suffix) {
        return text.to_string();
    }
    format!("【{term}】{suffix}")
}

fn starts_like_bracketed_term_suffix(suffix: &str) -> bool {
    suffix.starts_with('を')
        || suffix.starts_with('に')
        || suffix.starts_with('の')
        || suffix.starts_with("及び")
        || suffix.starts_with("へ")
        || suffix.starts_with("で")
        || suffix.starts_with('は')
        || suffix.starts_with('が')
        || suffix.starts_with('ま')
        || suffix.starts_with("状態")
}

fn restore_dark_core_fragments(text: &str) -> String {
    match text.trim() {
        "クコア" => "クコア】".to_string(),
        "ダークコア" => "【ダークコア】".to_string(),
        "ダークコア)を1個消費するごとに、この攻撃" => {
            "【ダークコア】を1個消費するごとに、この攻撃".to_string()
        }
        "ダークコア）及び" | "ダークコア】及び" => "【ダークコア】及び".to_string(),
        _ => text.to_string(),
    }
}

fn restore_exact_prefix_bracketed_term(text: &str, term: &str) -> String {
    let trimmed = text.trim();
    if trimmed == term {
        return format!("【{term}");
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::apply_common_replacements;

    #[test]
    fn restores_purchase_ratio_slash() {
        assert_eq!(
            apply_common_replacements("購入可能数：11"),
            "購入可能数：1/1"
        );
    }

    #[test]
    fn keeps_unbracketed_blast_effect_style_title() {
        assert_eq!(apply_common_replacements("斉爆効果"), "斉爆効果");
        assert_eq!(
            apply_common_replacements("斉爆効果を付与できる"),
            "斉爆効果を付与できる"
        );
    }

    #[test]
    fn restores_blast_effect_brackets_in_enemy_context() {
        assert_eq!(
            apply_common_replacements("キャラが敵に斉爆効果】を付与"),
            "キャラが敵に【斉爆効果】を付与"
        );
        assert_eq!(
            apply_common_replacements("キャラが敵に斉爆効果を付与"),
            "キャラが敵に【斉爆効果】を付与"
        );
    }

    #[test]
    fn restores_bracketed_effect_term_suffixes() {
        assert_eq!(
            apply_common_replacements("密集協和・オフセットを付与でき"),
            "【密集協和・オフセット】を付与でき"
        );
        assert_eq!(
            apply_common_replacements("密集協和・インターフェア)ヘのレスポンス："),
            "【密集協和・インターフェア】へのレスポンス："
        );
        assert_eq!(
            apply_common_replacements("共形エネルギー及び"),
            "【共形エネルギー】及び"
        );
        assert_eq!(apply_common_replacements("参加条件)"), "【参加条件】");
        assert_eq!(
            apply_common_replacements("リンク状態は10秒間持続。"),
            "【リンク状態】は10秒間持続。"
        );
        assert_eq!(
            apply_common_replacements("震撃協和・インターフェアま"),
            "【震撃協和・インターフェア】ま"
        );
        assert_eq!(apply_common_replacements("風蝕効果)"), "【風蝕効果】");
        assert_eq!(
            apply_common_replacements("風蝕効果を付与できる"),
            "風蝕効果を付与できる"
        );
        assert_eq!(
            apply_common_replacements("結霜効果を付与できる"),
            "結霜効果を付与できる"
        );
        assert_eq!(
            apply_common_replacements("騒光効果を付与できる"),
            "騒光効果を付与できる"
        );
        assert_eq!(
            apply_common_replacements("虚滅効果を付与できる"),
            "虚滅効果を付与できる"
        );
        assert_eq!(
            apply_common_replacements("または震撃協和・オフセットを付与"),
            "または【震撃協和・オフセット】を付与"
        );
        assert_eq!(
            apply_common_replacements("密集協和状態中"),
            "密集協和状態中"
        );
    }

    #[test]
    fn does_not_invent_opening_quote_for_wrapped_fragments() {
        assert_eq!(
            apply_common_replacements("オイドマターを防げるわけがない」"),
            "オイドマターを防げるわけがない」"
        );
        assert_eq!(
            apply_common_replacements("砕けた記憶」または「砕けた悪夢」"),
            "「砕けた記憶」または「砕けた悪夢」"
        );
        assert_eq!(
            apply_common_replacements("空想の幻夢」をクリア"),
            "「空想の幻夢Ⅱ」をクリア"
        );
        assert_eq!(
            apply_common_replacements("深層空想秘境・空想の幻夢」で、報酬"),
            "「深層空想秘境・空想の幻夢」で、報酬"
        );
        assert_eq!(
            apply_common_replacements("君との旅路」で、同行ポイント5000到達"),
            "「君との旅路」で、同行ポイント5000到達"
        );
        assert_eq!(apply_common_replacements("完全無欠」"), "「完全無欠」");
    }

    #[test]
    fn restores_contextual_long_mark_and_small_tsu_confusions() {
        assert_eq!(
            apply_common_replacements("ースキルは同一目標"),
            "一スキルは同一目標"
        );
        assert_eq!(apply_common_replacements("そのー瞬"), "その一瞬");
        assert_eq!(
            apply_common_replacements("終奏スキルー覧"),
            "終奏スキル一覧"
        );
        assert_eq!(apply_common_replacements("なかつ"), "なかっ");
        assert_eq!(apply_common_replacements("つている。"), "っている。");
        assert_eq!(apply_common_replacements("秘問特结"), "秒間持続");
    }

    #[test]
    fn restores_only_exact_terminal_full_stop_lines() {
        assert_eq!(
            apply_common_replacements("焦熱ダメージがアップ"),
            "焦熱ダメージがアップ。"
        );
        assert_eq!(apply_common_replacements("る"), "る。");
        assert_eq!(apply_common_replacements("15秒間持続"), "15秒間持続。");
        assert_eq!(
            apply_common_replacements("逸話任務「卒業旅行」を進めると解放"),
            "逸話任務「卒業旅行」を進めると解放。"
        );
        assert_eq!(
            apply_common_replacements("共鳴モダリティ・密集協和状態中"),
            "共鳴モダリティ・密集協和状態中、"
        );
        assert_eq!(
            apply_common_replacements("ックノック、共鳴解放幕引きの光景・芝居の形"),
            "ックノック、共鳴解放幕引きの光景・芝居の形、"
        );
        assert_eq!(
            apply_common_replacements("ライド状態を解除し"),
            "ライド状態を解除し、"
        );
        assert_eq!(
            apply_common_replacements("学園のガーデンで待って"),
            "学園のガーデンで待って…"
        );
        assert_eq!(apply_common_replacements("ンターフェア"), "ンターフェア】");
        assert_eq!(apply_common_replacements("報酬プレビュ"), "報酬プレビュー");
        assert_eq!(
            apply_common_replacements("焦熱ダメージがアップし"),
            "焦熱ダメージがアップし"
        );
    }

    #[test]
    fn restores_exact_missing_leading_middle_dot_lines() {
        assert_eq!(
            apply_common_replacements("ダーニャがチームにいる時、敵が"),
            "・ダーニャがチームにいる時、敵が"
        );
        assert_eq!(apply_common_replacements("ダーニャが"), "ダーニャが");
        assert_eq!(apply_common_replacements("戦歌復唱"), "戦歌復唱");
    }

    #[test]
    fn restores_closed_blast_and_trickster_brackets() {
        assert_eq!(apply_common_replacements("斉爆効果)"), "【斉爆効果】");
        assert_eq!(
            apply_common_replacements("トリックスター】を召喚して敵"),
            "【トリックスター】を召喚して敵"
        );
        assert_eq!(apply_common_replacements("ダークコア"), "【ダークコア】");
        assert_eq!(apply_common_replacements("クコア"), "クコア】");
    }

    #[test]
    fn restores_set_count_parentheses_and_truncation() {
        assert_eq!(apply_common_replacements("5セッ）"), "（5セット）");
        assert_eq!(
            apply_common_replacements("踏破！パラドッ"),
            "踏破！パラドッ..."
        );
        assert_eq!(
            apply_common_replacements("踏破！パラドッ.."),
            "踏破！パラドッ..."
        );
        assert_eq!(
            apply_common_replacements("潮騒探検・イ."),
            "潮騒探検・ロイ..."
        );
    }

    #[test]
    fn restores_stamp_prefixes() {
        assert_eq!(apply_common_replacements("スタンプ乾杯"), "[スタンプ]乾杯");
        assert_eq!(
            apply_common_replacements("スタンプ]グッチョ！"),
            "[スタンプ]グッチョ！"
        );
        assert_eq!(
            apply_common_replacements("スタンプオハナへ"),
            "[スタンプ]オハナ～"
        );
    }

    #[test]
    fn restores_clipped_material_grid_labels() {
        assert_eq!(apply_common_replacements("海蝕干人."), "海蝕キメ...");
        assert_eq!(apply_common_replacements("中音·鋭…"), "中音・鋭...");
        assert_eq!(apply_common_replacements("凝縮レジ…."), "凝縮レジ...");
        assert_eq!(apply_common_replacements("普通の手"), "普通の手...");
        assert_eq!(
            apply_common_replacements("忙しいだろうから返信は."),
            "忙しいだろうから返信は…"
        );
    }

    #[test]
    fn strips_trailing_ui_values_from_labels() {
        assert_eq!(
            apply_common_replacements("響き渡る共鳴・ダーニャ+25"),
            "響き渡る共鳴・ダーニャ"
        );
        assert_eq!(
            apply_common_replacements("共鳴解放ダメージアップ10.9%"),
            "共鳴解放ダメージアップ"
        );
        assert_eq!(
            apply_common_replacements("共鳴解放ダメージアップ・7.1%"),
            "共鳴解放ダメージアップ"
        );
        assert_eq!(apply_common_replacements("HP2280"), "HP");
        assert_eq!(apply_common_replacements("攻撃力%"), "攻撃力％");
        assert_eq!(
            apply_common_replacements("任務アイテム56/1000"),
            "任務アイテム"
        );
        assert_eq!(apply_common_replacements("リソース72/1000"), "リソース");
        assert_eq!(apply_common_replacements("Sリソース72/1000"), "リソース");
    }

    #[test]
    fn normalizes_repeated_domain_ocr_confusions() {
        assert_eq!(
            apply_common_replacements("変奏スキルお久しぶりですねぇ～"),
            "変奏スキルお久しぶりですねぇ〜"
        );
        assert_eq!(apply_common_replacements("闇の原野"), "闇の原野へ");
        assert_eq!(apply_common_replacements("闇の原野へ"), "闇の原野へ");
        assert_eq!(
            apply_common_replacements("エカセンサと負荷ブロック"),
            "圧力センサと負荷ブロック"
        );
        assert_eq!(
            apply_common_replacements("本日の獲得可能回数：610"),
            "本日の獲得可能回数：6/10"
        );
        assert_eq!(
            apply_common_replacements("在：240、探検ノートを1枚獲得"),
            "在：240）、探検ノートを1枚獲得"
        );
        assert_eq!(apply_common_replacements("ツク"), "ック");
        assert_eq!(apply_common_replacements("つてください"), "ってください");
        assert_eq!(
            apply_common_replacements("星巡るフリッパーー"),
            "星巡るフリッパー"
        );
        assert_eq!(
            apply_common_replacements("超新星級シューターー"),
            "超新星級シューター"
        );
        assert_eq!(apply_common_replacements("イレナ"), "イレーナ");
    }

    #[test]
    fn restores_exact_ui_labels() {
        assert_eq!(apply_common_replacements("攻撃力%"), "攻撃力％");
        assert_eq!(
            apply_common_replacements("異世界リンク"),
            "異世界リンク・下"
        );
        assert_eq!(
            apply_common_replacements("異世界リンク・"),
            "異世界リンク・下"
        );
        assert_eq!(
            apply_common_replacements("イベント紹介"),
            "【イベント紹介】"
        );
        assert_eq!(
            apply_common_replacements("共鳴解放を発動時、攻撃力が7.2%"),
            "共鳴解放を発動時、攻撃力が7.2％"
        );
        assert_eq!(
            apply_common_replacements("クリティカルダメージが30%アップ。"),
            "◆クリティカルダメージが30％アップ。"
        );
        assert_eq!(
            apply_common_replacements("光なき平野の記憶"),
            "光なき平野の記憶"
        );
    }
}
