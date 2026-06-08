//! Offline Japanese dictionary lookup over OCR'd text.
//!
//! Segmentation and per-morpheme lemmatization are done by Lindera (embedded IPADIC, see
//! [`crate::morphology`]): each line is split into morphemes carrying a
//! dictionary (base) form. Multi-morpheme spans are then resolved through a
//! recursive deinflection layer and the bundled JMdict lexicon
//! (scriptin/jmdict-simplified, CC BY-SA 4.0) for glosses.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::frequency::FrequencyTable;
use crate::morphology::{Analyzer, Morpheme};
use crate::pos::{LinderaPos, PosClass};
use crate::script::{is_kana, is_katakana};

mod deinflection;
mod raw;

use deinflection::{deinflect, entry_matches_type};
use raw::RawWord;

/// Most consecutive morphemes a single token may span (二人, 朝ご飯, 手に入れる,
/// …). Bounds the lattice search.
const MAX_COMPOUND_MORPHEMES: usize = 6;

/// Cost added per token in the segmentation lattice. A small positive bias
/// toward fewer tokens; kept low so that splitting a rare false compound into
/// frequent function morphemes (してき -> し+て+き) still wins.
const TOKEN_PENALTY: f32 = 1.0;
/// Cost for grouped non-dictionary punctuation/digit runs when refining one
/// long unknown OCR chunk. It is deliberately high: known terms should dominate
/// the split, and unresolved names should stay as one unknown token.
const UNKNOWN_RESEGMENT_COST: f32 = 20.0;

#[derive(Debug, Clone, Copy)]
struct RubySpec {
    text: &'static str,
    furigana: Option<&'static str>,
}

const fn ruby(text: &'static str, furigana: Option<&'static str>) -> RubySpec {
    RubySpec { text, furigana }
}

#[derive(Debug, Clone, Copy)]
struct KnownTermSpec {
    surface: &'static str,
    dictionary_form: &'static str,
    part_of_speech: &'static [&'static str],
    ruby: &'static [RubySpec],
    glosses: &'static [&'static str],
}

// Wuthering Waves and game-UI terms that are absent from JMdict or need a
// domain-specific display. These are intentionally small, exact overlays on top
// of JMdict: the base dictionary still owns ordinary Japanese segmentation.
//
// Learner-facing canonical forms follow docs/eval-metadata.md. In particular,
// usually-kana vocabulary should use the kana spelling as the primary
// dictionary_form even when a kanji form exists in JMdict.
const DOMAIN_KNOWN_TERMS: &[KnownTermSpec] = &[
    KnownTermSpec {
        surface: "購入可能数",
        dictionary_form: "購入可能数",
        part_of_speech: &["n"],
        ruby: &[
            ruby("購入", Some("こうにゅう")),
            ruby("可能", Some("かのう")),
            ruby("数", Some("すう")),
        ],
        glosses: &["available purchase count; purchasable quantity  (n)"],
    },
    KnownTermSpec {
        surface: "秒間",
        dictionary_form: "秒間",
        part_of_speech: &["n-suf"],
        ruby: &[ruby("秒間", Some("びょうかん"))],
        glosses: &["for ... seconds; interval measured in seconds  (n-suf)"],
    },
    KnownTermSpec {
        surface: "時間",
        dictionary_form: "時間",
        part_of_speech: &["n"],
        ruby: &[ruby("時間", Some("じかん"))],
        glosses: &["time; hour  (n)"],
    },
    KnownTermSpec {
        surface: "時",
        dictionary_form: "時",
        part_of_speech: &["n"],
        ruby: &[ruby("時", Some("とき"))],
        glosses: &["time; moment; occasion  (n)"],
    },
    KnownTermSpec {
        surface: "セット",
        dictionary_form: "セット",
        part_of_speech: &["n"],
        ruby: &[ruby("セット", None)],
        glosses: &["set  (n)"],
    },
    KnownTermSpec {
        surface: "ラウンド",
        dictionary_form: "ラウンド",
        part_of_speech: &["n"],
        ruby: &[ruby("ラウンド", None)],
        glosses: &["round  (n)"],
    },
    KnownTermSpec {
        surface: "エンド",
        dictionary_form: "エンド",
        part_of_speech: &["n"],
        ruby: &[ruby("エンド", None)],
        glosses: &["end  (n)"],
    },
    KnownTermSpec {
        surface: "リスト",
        dictionary_form: "リスト",
        part_of_speech: &["n"],
        ruby: &[ruby("リスト", None)],
        glosses: &["list  (n)"],
    },
    KnownTermSpec {
        surface: "リンク状態",
        dictionary_form: "リンク状態",
        part_of_speech: &["n"],
        ruby: &[ruby("リンク", None), ruby("状態", Some("じょうたい"))],
        glosses: &["link state  (n)"],
    },
    // See docs/eval-metadata.md: these exact lexicalized nouns are clearer
    // learner primaries than treating the same surface as a verb continuative
    // stem. Keep this list narrow; broad deverbal-noun promotion mislabels
    // common verb stems such as し, して, 行い, and 削り.
    KnownTermSpec {
        surface: "誓い",
        dictionary_form: "誓い",
        part_of_speech: &["n"],
        ruby: &[ruby("誓い", Some("ちかい"))],
        glosses: &["oath; vow; pledge  (n)"],
    },
    KnownTermSpec {
        surface: "まどろみ",
        dictionary_form: "まどろみ",
        part_of_speech: &["n"],
        ruby: &[ruby("まどろみ", None)],
        glosses: &["doze; nap; slumber  (n)"],
    },
    KnownTermSpec {
        surface: "轟き",
        dictionary_form: "轟き",
        part_of_speech: &["n"],
        ruby: &[ruby("轟き", Some("とどろき"))],
        glosses: &["roar; peal; rumble; booming  (n)"],
    },
    KnownTermSpec {
        surface: "いざない",
        dictionary_form: "誘い",
        part_of_speech: &["n"],
        ruby: &[ruby("いざない", None)],
        glosses: &["invitation; call; lure  (n)"],
    },
    KnownTermSpec {
        surface: "導き",
        dictionary_form: "導き",
        part_of_speech: &["n"],
        ruby: &[ruby("導き", Some("みちびき"))],
        glosses: &["guidance; direction; leading  (n)"],
    },
    KnownTermSpec {
        surface: "一定",
        dictionary_form: "一定",
        part_of_speech: &["adj-no", "n", "vs"],
        ruby: &[ruby("一定", Some("いってい"))],
        glosses: &[
            "fixed; settled; constant; definite; uniform; regular; defined; standardized  (adj-no, n, vs)",
        ],
    },
    KnownTermSpec {
        surface: "奇点",
        dictionary_form: "奇点",
        part_of_speech: &["n"],
        ruby: &[ruby("奇点", Some("きてん"))],
        glosses: &["singular point; singularity  (n)"],
    },
    KnownTermSpec {
        surface: "戦歌",
        dictionary_form: "戦歌",
        part_of_speech: &["n"],
        ruby: &[ruby("戦歌", Some("せんか"))],
        glosses: &["war song; battle song  (n)"],
    },
    KnownTermSpec {
        surface: "無力化",
        dictionary_form: "無力化",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("無力化", Some("むりょくか"))],
        glosses: &["neutralization; making powerless; disabling  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "攻撃",
        dictionary_form: "攻撃",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("攻撃", Some("こうげき"))],
        glosses: &["attack; assault; strike  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "攻撃力",
        dictionary_form: "攻撃力",
        part_of_speech: &["n"],
        ruby: &[ruby("攻撃力", Some("こうげきりょく"))],
        glosses: &["attack power; offensive strength  (n)"],
    },
    KnownTermSpec {
        surface: "重撃",
        dictionary_form: "重撃",
        part_of_speech: &["n"],
        ruby: &[ruby("重撃", Some("じゅうげき"))],
        glosses: &["heavy attack; heavy strike  (n)"],
    },
    KnownTermSpec {
        surface: "効果",
        dictionary_form: "効果",
        part_of_speech: &["n", "adj-no"],
        ruby: &[ruby("効果", Some("こうか"))],
        glosses: &["effect; effectiveness; result  (n, adj-no)"],
    },
    KnownTermSpec {
        surface: "数",
        dictionary_form: "数",
        part_of_speech: &["n"],
        ruby: &[ruby("数", Some("かず"))],
        glosses: &["number; amount  (n)"],
    },
    KnownTermSpec {
        surface: "必要",
        dictionary_form: "必要",
        part_of_speech: &["n", "adj-na"],
        ruby: &[ruby("必要", Some("ひつよう"))],
        glosses: &["necessity; need; requirement  (n, adj-na)"],
    },
    KnownTermSpec {
        surface: "特定",
        dictionary_form: "特定",
        part_of_speech: &["n", "vs", "vt", "adj-no"],
        ruby: &[ruby("特定", Some("とくてい"))],
        glosses: &["specifying; identifying; pinpointing  (n, vs, vt, adj-no)"],
    },
    KnownTermSpec {
        surface: "発動",
        dictionary_form: "発動",
        part_of_speech: &["n", "vs", "vt", "vi"],
        ruby: &[ruby("発動", Some("はつどう"))],
        glosses: &["activation; invocation; triggering  (n, vs, vt, vi)"],
    },
    KnownTermSpec {
        surface: "解放",
        dictionary_form: "解放",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("解放", Some("かいほう"))],
        glosses: &["release; liberation; unleashing  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "獲得",
        dictionary_form: "獲得",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("獲得", Some("かくとく"))],
        glosses: &["acquisition; obtaining; gaining  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "付与",
        dictionary_form: "付与",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("付与", Some("ふよ"))],
        glosses: &["grant; bestowal; endowment  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "形",
        dictionary_form: "形",
        part_of_speech: &["n"],
        ruby: &[ruby("形", Some("かたち"))],
        glosses: &["form; shape; figure  (n)"],
    },
    KnownTermSpec {
        surface: "素材",
        dictionary_form: "素材",
        part_of_speech: &["n"],
        ruby: &[ruby("素材", Some("そざい"))],
        glosses: &["material; ingredient; resource  (n)"],
    },
    KnownTermSpec {
        surface: "レベル",
        dictionary_form: "レベル",
        part_of_speech: &["n"],
        ruby: &[ruby("レベル", None)],
        glosses: &["level; grade; standard  (n)"],
    },
    KnownTermSpec {
        surface: "強化",
        dictionary_form: "強化",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("強化", Some("きょうか"))],
        glosses: &["strengthening; enhancement  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "アップ",
        dictionary_form: "アップ",
        part_of_speech: &["n", "n-suf", "vs", "vt", "vi"],
        ruby: &[ruby("アップ", None)],
        glosses: &["rise; increase; raising; lifting  (n, n-suf, vs, vt, vi)"],
    },
    KnownTermSpec {
        surface: "通常",
        dictionary_form: "通常",
        part_of_speech: &["adj-no", "n", "adv"],
        ruby: &[ruby("通常", Some("つうじょう"))],
        glosses: &["usual; normal; ordinary  (adj-no, n, adv)"],
    },
    KnownTermSpec {
        surface: "チーム内",
        dictionary_form: "チーム内",
        part_of_speech: &["n"],
        ruby: &[ruby("チーム", None), ruby("内", Some("ない"))],
        glosses: &["within the team; team-internal  (n)"],
    },
    KnownTermSpec {
        surface: "協奏",
        dictionary_form: "協奏",
        part_of_speech: &["n", "vs"],
        ruby: &[ruby("協奏", Some("きょうそう"))],
        glosses: &["concerto; concerted performance  (n, vs)"],
    },
    KnownTermSpec {
        surface: "変奏",
        dictionary_form: "変奏",
        part_of_speech: &["n", "vs"],
        ruby: &[ruby("変奏", Some("へんそう"))],
        glosses: &["variation; playing a variation  (n, vs)"],
    },
    KnownTermSpec {
        surface: "終奏",
        dictionary_form: "終奏",
        part_of_speech: &["n"],
        ruby: &[ruby("終奏", Some("しゅうそう"))],
        glosses: &["postlude; ending performance  (n)"],
    },
    KnownTermSpec {
        surface: "共鳴",
        dictionary_form: "共鳴",
        part_of_speech: &["n", "vs", "vi"],
        ruby: &[ruby("共鳴", Some("きょうめい"))],
        glosses: &["resonance  (n, vs, vi)"],
    },
    KnownTermSpec {
        surface: "回路",
        dictionary_form: "回路",
        part_of_speech: &["n"],
        ruby: &[ruby("回路", Some("かいろ"))],
        glosses: &["circuit; cycle; loop  (n)"],
    },
    KnownTermSpec {
        surface: "幻滅",
        dictionary_form: "幻滅",
        part_of_speech: &["n", "vs", "vi"],
        ruby: &[ruby("幻滅", Some("げんめつ"))],
        glosses: &["disillusionment; disenchantment  (n, vs, vi)"],
    },
    KnownTermSpec {
        surface: "破壊",
        dictionary_form: "破壊",
        part_of_speech: &["n", "vs", "vt", "vi"],
        ruby: &[ruby("破壊", Some("はかい"))],
        glosses: &["destruction; disruption  (n, vs, vt, vi)"],
    },
    KnownTermSpec {
        surface: "波模様",
        dictionary_form: "波模様",
        part_of_speech: &["n"],
        ruby: &[ruby("波", Some("なみ")), ruby("模様", Some("もよう"))],
        glosses: &["wave pattern  (n)"],
    },
    KnownTermSpec {
        surface: "栄養液",
        dictionary_form: "栄養液",
        part_of_speech: &["n"],
        ruby: &[ruby("栄養液", Some("えいようえき"))],
        glosses: &["nutrient solution; nourishing liquid  (n)"],
    },
    KnownTermSpec {
        surface: "所持中",
        dictionary_form: "所持中",
        part_of_speech: &["exp"],
        ruby: &[ruby("所持", Some("しょじ")), ruby("中", Some("ちゅう"))],
        glosses: &["currently owned; in possession"],
    },
    KnownTermSpec {
        surface: "組織長",
        dictionary_form: "組織長",
        part_of_speech: &["n"],
        ruby: &[ruby("組織長", Some("そしきちょう"))],
        glosses: &["head of an organization; organization leader  (n)"],
    },
    KnownTermSpec {
        surface: "令尹",
        dictionary_form: "令尹",
        part_of_speech: &["n"],
        ruby: &[ruby("令尹", Some("れいいん"))],
        glosses: &["ancient Chinese official title; chief magistrate; governor  (n)"],
    },
    KnownTermSpec {
        surface: "ドロップ率",
        dictionary_form: "ドロップ率",
        part_of_speech: &["n"],
        ruby: &[ruby("ドロップ", None), ruby("率", Some("りつ"))],
        glosses: &["drop rate; loot drop rate  (game UI noun)"],
    },
    KnownTermSpec {
        surface: "ドロップ",
        dictionary_form: "ドロップ",
        part_of_speech: &["n"],
        ruby: &[ruby("ドロップ", None)],
        glosses: &["drop; loot drop  (game UI noun)"],
    },
    KnownTermSpec {
        surface: "特級",
        dictionary_form: "特級",
        part_of_speech: &["n", "adj-no"],
        ruby: &[ruby("特級", Some("とっきゅう"))],
        glosses: &["special grade; highest class  (n, adj-no)"],
    },
    KnownTermSpec {
        surface: "凝縮ダメージ",
        dictionary_form: "凝縮ダメージ",
        part_of_speech: &["n"],
        ruby: &[ruby("凝縮", Some("ぎょうしゅく")), ruby("ダメージ", None)],
        glosses: &["Glacio damage; condensation damage  (n)"],
    },
    KnownTermSpec {
        surface: "消滅ダメージ",
        dictionary_form: "消滅ダメージ",
        part_of_speech: &["n"],
        ruby: &[ruby("消滅", Some("しょうめつ")), ruby("ダメージ", None)],
        glosses: &["Havoc damage; annihilation damage  (n)"],
    },
    KnownTermSpec {
        surface: "共振度",
        dictionary_form: "共振度",
        part_of_speech: &["n"],
        ruby: &[ruby("共振度", Some("きょうしんど"))],
        glosses: &["resonance gauge; vibration strength gauge  (n)"],
    },
    KnownTermSpec {
        surface: "図鑑",
        dictionary_form: "図鑑",
        part_of_speech: &["n"],
        ruby: &[ruby("図鑑", Some("ずかん"))],
        glosses: &["illustrated reference book; illustrated encyclopedia  (n)"],
    },
    KnownTermSpec {
        surface: "デフォルト",
        dictionary_form: "デフォルト",
        part_of_speech: &["n"],
        ruby: &[ruby("デフォルト", None)],
        glosses: &["default  (n)"],
    },
    KnownTermSpec {
        surface: "クリア",
        dictionary_form: "クリア",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("クリア", None)],
        glosses: &["clearance; clearing; completing a game or objective  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "装備",
        dictionary_form: "装備",
        part_of_speech: &["n", "vs", "vt"],
        ruby: &[ruby("装備", Some("そうび"))],
        glosses: &["equipment; outfit; to equip  (n, vs, vt)"],
    },
    KnownTermSpec {
        surface: "売り切れ",
        dictionary_form: "売り切れる",
        part_of_speech: &["v1", "vi"],
        ruby: &[
            ruby("売", Some("う")),
            ruby("り", None),
            ruby("切", Some("き")),
            ruby("れる", None),
        ],
        glosses: &["to be sold out  (v1, vi)"],
    },
    KnownTermSpec {
        surface: "ショップ",
        dictionary_form: "ショップ",
        part_of_speech: &["n"],
        ruby: &[ruby("ショップ", None)],
        glosses: &["shop; store  (n)"],
    },
    KnownTermSpec {
        surface: "共鳴者",
        dictionary_form: "共鳴者",
        part_of_speech: &["n"],
        ruby: &[ruby("共鳴者", Some("きょうめいしゃ"))],
        glosses: &["resonator (Wuthering Waves term)"],
    },
    KnownTermSpec {
        surface: "敵",
        dictionary_form: "敵",
        part_of_speech: &["n"],
        ruby: &[ruby("敵", Some("てき"))],
        glosses: &["enemy; opponent; adversary  (n)"],
    },
    KnownTermSpec {
        surface: "目標",
        dictionary_form: "目標",
        part_of_speech: &["n"],
        ruby: &[ruby("目標", Some("もくひょう"))],
        glosses: &["target; objective; goal  (n)"],
    },
    KnownTermSpec {
        surface: "できる",
        dictionary_form: "できる",
        part_of_speech: &["v1", "vi"],
        ruby: &[ruby("できる", None)],
        glosses: &["to be able to; can  (v1, vi)"],
    },
    KnownTermSpec {
        surface: "いる",
        dictionary_form: "いる",
        part_of_speech: &["v1", "vi"],
        ruby: &[ruby("いる", None)],
        glosses: &["to be; to exist; to stay  (v1, vi)"],
    },
    KnownTermSpec {
        surface: "たち",
        dictionary_form: "たち",
        part_of_speech: &["suf"],
        ruby: &[ruby("たち", None)],
        glosses: &["pluralizing suffix; and others  (suf)"],
    },
];

const DOMAIN_UNKNOWN_TERMS: &[&str] = &[
    "ヴォイドマター粒子",
    "喧騒に隠す回光",
    "命理崩壊の弦",
    "エーテル・レゾナンス",
    "インフェルノ・シャドウ",
    "リフレクト・ブレイズ",
    "ダメージブースト",
    "ヴォイドストーム",
    "パティナ・フォーム",
    "ロスト・ドリーム",
    "絶えない余韻",
    "空を切り裂く冥雷",
    "山を轟かせる崩火",
    "ミッドナイト・ベール",
    "二度と輝かない沈日",
    "闇を取り払う浮星",
    "谷を突き抜ける長風",
    "夜にこびり付く白霜",
    "月を窺う軽雲",
    "アストロ・ロード",
    "ロードビルダー",
    "リンクドスパイン",
    "スタートーチ学園",
    "スタートーチ",
    "残星組織",
    "ブラックショア",
    "セブン・ヒルズ",
    "拾方薬局",
    "スペーストレック",
    "フラクトシデス",
    "斉爆効果",
    "ソラランク",
    "ソラランクアップ",
    "ソラリス",
    "リナシータ",
    "乗霄山",
    "今州",
    "逆境深塔",
    "深塔",
    "ダークコア",
    "ラハイロイ",
    "ナスターシャ",
    "ブラント",
    "グリフェックス",
    "マウントギャラル",
    "異夢",
    "クールタイム",
    "共形エネルギー",
    "ヴォイドマター",
    "ヴォイドマター粒",
    "オイドマター",
    "ヴォイドスペース",
    "気動",
    "騒光",
    "騒光効果",
    "結霜効果",
    "虚滅効果",
    "震撃",
    "震撃協和",
    "無妄者",
    "無冠者",
    "異想音骸",
    // Rare general word, but in this corpus it is a Wuthering Waves event term.
    // See docs/eval-metadata.md: narrow domain unknowns can suppress misleading
    // dictionary popups when the ordinary reading is unlikely to help a learner.
    "潮音",
    "雲閃",
    "炎騎",
    "鳴鐘の亀",
    "津波級",
    "スカー・異生のナイトメア",
    "シャドウステッパー",
    "ホロタクティクス",
    "ホロタクティク...",
    "ホロタクティクス・ファントムペイ",
    "無音区",
    "金髄",
    "集域",
    "シーベッド",
    "エイメス",
    "ウェーブライン",
    "シグリカ",
    "イレーナ",
    "ダーニャ",
    "モーニエ",
    "ディリファ",
    "リンネー",
    "ファントムモス",
    "アレフ1",
    "伊藤美来",
    "CV：",
    "ノックノック",
    "星巡りの調べ",
    "ソラガイド",
    "B.1.N.G.O.",
    "達成数",
    "リスト",
    "共鳴者EXP",
    "武器EXP",
    "フェンリコ",
    "ハイヴェイシャ",
    "哀切の凶鳥",
    "無情のサギ",
    "輝き蛍の軍勢",
    "機械アボミネーション",
    "フェイタルエラー",
    "ナイトメア・輝き蛍の軍勢",
    "ナイトメア・哀切の凶鳥",
    "ナイトメア・雷刹のウロコ",
    "ブラインドフォール・残",
    "リバースプレイン・残像",
    "スタングナントラン・残",
    "像集落",
    "ヴェイシャ",
    "ルールドリス",
    "のドレイク",
    "ト霊",
    "長離",
    "凌陽",
    "鑑心",
    "灯灯",
    "金陽鳳",
    "燕雀菓",
    "雲芝",
    "スペーストレック・コレクティブ",
    "ギンヌンガミール",
    "ヴォイドス",
    "青実",
    "唱喚",
    "機花音核",
    "レジ...",
    "イン...",
    "砕晶",
    "虹鎮",
    "残振",
    "鍛潮",
    "幻相",
    "クロックリスク",
    "走声蝶",
    "音匣",
    "データドック",
    "トゲバラタケ",
    "集燃体",
    "メカパーツチェーン",
    "4段目",
    "3段目",
    "2段目",
    "1段目",
    "音骸",
    "終奏",
    "斉爆",
    "音核",
    "瑝瓏",
    "凝素",
    "星声",
    "シェルコイン",
    "ソラ",
    "ブブ",
    "声律",
    "キメ...",
    "マ...",
    "手...",
    "集...",
    "叫...",
    "唸...",
    "侵...",
    "海...",
    "切...",
    "機...",
    "鋭...",
    "1回",
];

/// A single dictionary sense: its parts of speech and English glosses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sense {
    pub part_of_speech: Vec<String>,
    pub glosses: Vec<String>,
    /// JMdict `misc` tags (e.g. `arch`, `obs`, `rare`, `uk`). Used to demote
    /// archaic/obscure senses when ordering glosses for display.
    pub misc: Vec<String>,
}

/// Custom display content for a domain entry whose popup should not be derived
/// from one flat JMdict-style reading/gloss list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PopupOverride {
    pub ruby: Vec<RubySegment>,
    pub glosses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RubySegment {
    pub text: String,
    pub furigana: Option<String>,
}

/// One dictionary entry: written (kanji) forms, readings (kana), and senses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kanji: Vec<String>,
    pub kana: Vec<String>,
    pub senses: Vec<Sense>,
    /// True if any kanji/kana form is flagged "common" in JMdict. Used to bias
    /// segmentation away from rare homographs.
    pub common: bool,
    /// Optional domain-specific popup display data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popup_override: Option<PopupOverride>,
}

impl Entry {
    /// The most representative headword: first kanji form, else first reading.
    pub fn headword(&self) -> &str {
        self.kanji
            .first()
            .or_else(|| self.kana.first())
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// An in-memory JMdict index keyed by every surface form (kanji and kana),
/// paired with a Lindera analyzer for segmentation + lemmatization.
pub struct Dictionary {
    entries: Vec<Entry>,
    by_form: HashMap<String, Vec<usize>>,
    analyzer: Analyzer,
    frequency: FrequencyTable,
}

impl Dictionary {
    fn with_parts(analyzer: Analyzer, frequency: FrequencyTable) -> Self {
        Self {
            entries: Vec::new(),
            by_form: HashMap::new(),
            analyzer,
            frequency,
        }
    }

    /// Stream-parse a jmdict-simplified JSON file. Each word entry sits on its
    /// own line, so we parse line by line instead of loading the whole file.
    /// Also loads the frequency table from a sibling `jpdb-freq/` directory (if
    /// present) to drive cost-based segmentation.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let analyzer = Analyzer::new().context("failed to initialize morphological analyzer")?;
        let path = path.as_ref();
        let frequency = load_sibling_frequency(path);
        let file = File::open(path)
            .with_context(|| format!("failed to open lexicon {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut dict = Dictionary::with_parts(analyzer, frequency);
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim().trim_end_matches(',');
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                continue;
            }
            // Header fragments and array brackets fail to parse and are skipped.
            let Ok(raw) = serde_json::from_str::<RawWord>(trimmed) else {
                continue;
            };
            if let Some(entry) = raw.into_entry() {
                dict.insert(entry);
            }
        }

        if dict.entries.is_empty() {
            anyhow::bail!(
                "no dictionary entries parsed from {}; is this a jmdict-simplified JSON file?",
                path.display()
            );
        }
        Ok(dict)
    }

    /// Build a dictionary directly from entries (used in tests). The frequency
    /// table is empty, so segmentation falls back to a fewest-tokens preference.
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        let analyzer = Analyzer::new().expect("load embedded IPADIC dictionary");
        let mut dict = Dictionary::with_parts(analyzer, FrequencyTable::empty());
        for entry in entries {
            dict.insert(entry);
        }
        dict
    }

    fn insert(&mut self, entry: Entry) {
        let index = self.entries.len();
        for form in entry.kanji.iter().chain(entry.kana.iter()) {
            self.by_form.entry(form.clone()).or_default().push(index);
        }
        self.entries.push(entry);
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether any JMdict entry is registered under this exact surface form.
    pub fn contains(&self, form: &str) -> bool {
        self.by_form.contains_key(form)
    }

    /// Whether any entry registered under this form is flagged common in JMdict.
    pub fn is_common(&self, form: &str) -> bool {
        self.entries_for(form)
            .is_some_and(|entries| entries.iter().any(|entry| entry.common))
    }

    pub fn is_domain_unknown_term(&self, surface: &str) -> bool {
        is_domain_unknown_surface(surface)
    }

    /// Entries registered under an exact surface form, if any.
    fn entries_for(&self, form: &str) -> Option<Vec<&Entry>> {
        let indices = self.by_form.get(form)?;
        Some(indices.iter().map(|&i| &self.entries[i]).collect())
    }

    /// Segment a line into morphemes (Lindera), then choose the minimum-cost
    /// segmentation over a candidate lattice by dynamic programming (Viterbi) —
    /// the same idea Lindera/MeCab use at the morpheme layer, lifted to the
    /// JMdict-term layer. Each single morpheme is a candidate node, as is every
    /// adjacent span whose fused form is a JMdict entry; node cost is
    /// `ln(frequency rank)` (rarer = costlier) plus a small per-token penalty.
    ///
    /// This fuses real compounds (聖遺物, 必殺技 — rare-but-real, so cheaper as
    /// one node than as rarer-summed pieces) yet refuses grammatical
    /// coincidences (一掃して + き -> してき "史的"): the function morphemes are so
    /// frequent that splitting beats the rare false compound. No hand-tuned
    /// common/POS gate — frequency does the disambiguation.
    pub fn analyze_line(&self, line: &str) -> Vec<Token> {
        let morphemes = match self.analyzer.analyze(line) {
            Ok(morphemes) => morphemes,
            Err(error) => {
                tracing::warn!("morphological analysis failed for {line:?}: {error:#}");
                return Vec::new();
            }
        };
        if morphemes.is_empty() {
            return Vec::new();
        }

        let count = morphemes.len();
        let mut best_cost = vec![f32::INFINITY; count + 1];
        let mut back: Vec<Option<(usize, Token)>> = (0..=count).map(|_| None).collect();
        best_cost[0] = 0.0;

        for end in 1..=count {
            let earliest = end.saturating_sub(MAX_COMPOUND_MORPHEMES);
            for start in earliest..end {
                if best_cost[start].is_infinite() {
                    continue;
                }
                let Some((cost, token)) = self.node(&morphemes, start, end) else {
                    continue;
                };
                let total = best_cost[start] + cost;
                if total < best_cost[end] {
                    best_cost[end] = total;
                    back[end] = Some((start, token));
                }
            }
        }

        // Backtrack the minimum-cost path into tokens (reading order).
        let mut tokens = Vec::new();
        let mut end = count;
        while end > 0 {
            let (start, token) = back[end].take().expect("every position is reachable");
            tokens.push(token);
            end = start;
        }
        tokens.reverse();
        let tokens = tokens
            .into_iter()
            .flat_map(|token| self.refine_unknown_token(token))
            .collect();
        let tokens = self.post_process_tokens(tokens);
        let tokens = cover_missing_unknown_tokens(line, tokens);
        let tokens = split_unknown_boundary_separator_tokens(tokens);
        self.split_ui_count_label_terms(tokens)
    }

    /// Cost and token for morpheme span `[start, end)`, or `None` when the span
    /// is not a valid lattice node. A single morpheme is always a node (resolved
    /// against JMdict, or an unknown token); a multi-morpheme span is a node only
    /// when its fused form (dictionary form first, then literal surface) is a
    /// JMdict entry that isn't a grammatical false-merge.
    fn node(&self, morphemes: &[Morpheme], start: usize, end: usize) -> Option<(f32, Token)> {
        let slice = &morphemes[start..end];
        let last = slice.last().expect("span is non-empty");
        let surface = span_surface(slice);

        if let Some(token) = self.polite_past_auxiliary_token(slice, &surface) {
            return Some((TOKEN_PENALTY, token));
        }
        match self.resolve_span(slice, &surface) {
            Some(resolution) => {
                if should_suppress_single_kana_content_lookup(slice, &surface, &resolution) {
                    return Some((TOKEN_PENALTY, unknown_surface_token(surface)));
                }
                if slice.len() > 1 && is_false_particle_merge(slice, &resolution.entries) {
                    return None;
                }
                let reasons = if !resolution.reasons.is_empty() {
                    resolution.reasons.clone()
                } else if resolution.matched_lemma && last.is_inflected() {
                    inflection_reasons(last)
                } else {
                    Vec::new()
                };
                let cost = self.frequency.cost(&resolution.form) + TOKEN_PENALTY;
                let token = Token {
                    surface,
                    dictionary_form: resolution.form,
                    reasons,
                    entries: ranked_entries(resolution.entries, last.major_pos()),
                    source_pos: None,
                    note_override: None,
                };
                Some((cost, token))
            }
            // A lone unknown morpheme is still a node (its own surface), so the
            // lattice can always reach the end; a longer unresolved span is not.
            None if slice.len() == 1 => {
                let cost = self.frequency.cost(&last.base_form) + TOKEN_PENALTY;
                Some((cost, unknown_token(last)))
            }
            None => None,
        }
    }

    fn polite_past_auxiliary_token(&self, slice: &[Morpheme], surface: &str) -> Option<Token> {
        if slice.len() != 2
            || slice[0].major_pos() != LinderaPos::AuxVerb
            || slice[1].major_pos() != LinderaPos::AuxVerb
            || slice[0].base_form != "ます"
            || slice[1].base_form != "た"
        {
            return None;
        }
        Some(Token {
            surface: surface.to_string(),
            dictionary_form: "ます".to_string(),
            reasons: vec!["丁寧".to_string(), "過去".to_string()],
            entries: Vec::new(),
            source_pos: Some(LinderaPos::AuxVerb),
            note_override: Some("Polite past auxiliary.".to_string()),
        })
    }

    /// Resolve a span against JMdict, preferring Lindera's lemma and literal
    /// surface before recursive deinflection. `surface` is the precomputed
    /// literal surface.
    fn resolve_span(&self, slice: &[Morpheme], surface: &str) -> Option<Resolution<'_>> {
        let lemma = span_lemma(slice);
        if let Some(resolution) = self.suru_stem_resolution(surface, &lemma) {
            return Some(resolution);
        }

        if let Some(entries) = self.entries_for(&lemma) {
            return Some(Resolution {
                form: lemma,
                entries,
                matched_lemma: true,
                reasons: Vec::new(),
            });
        }

        if let Some(entries) = self.entries_for(surface) {
            return Some(Resolution {
                form: surface.to_string(),
                entries,
                matched_lemma: false,
                reasons: Vec::new(),
            });
        }

        if can_deinflect_span(slice) {
            for candidate in deinflect(surface) {
                if let Some(entries) = self.entries_for(&candidate.form) {
                    let entries = entries
                        .into_iter()
                        .filter(|entry| entry_matches_type(entry, candidate.word_type))
                        .collect::<Vec<_>>();
                    if !entries.is_empty() {
                        return Some(Resolution {
                            form: candidate.form,
                            entries,
                            matched_lemma: true,
                            reasons: candidate.reasons,
                        });
                    }
                }
            }
        }

        None
    }

    fn suru_stem_resolution<'a>(
        &'a self,
        surface: &str,
        lindera_lemma: &str,
    ) -> Option<Resolution<'a>> {
        if !surface.ends_with('し') || !lindera_lemma.ends_with('す') {
            return None;
        }

        for candidate in deinflect(surface) {
            if !candidate.form.ends_with("する") {
                continue;
            }
            let Some(entries) = self.entries_for(&candidate.form) else {
                continue;
            };
            let entries = entries
                .into_iter()
                .filter(|entry| entry_matches_type(entry, candidate.word_type))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            return Some(Resolution {
                form: candidate.form,
                entries,
                matched_lemma: true,
                reasons: candidate.reasons,
            });
        }

        None
    }

    fn refine_unknown_token(&self, token: Token) -> Vec<Token> {
        if token.is_known() || !should_resegment_unknown_surface(&token.surface) {
            return vec![token];
        }
        self.resegment_unknown_surface(&token.surface)
            .filter(|tokens| should_accept_resegmented_unknown(&token.surface, tokens))
            .unwrap_or_else(|| vec![token])
    }

    fn resegment_unknown_surface(&self, surface: &str) -> Option<Vec<Token>> {
        let chars = surface.chars().collect::<Vec<_>>();
        let len = chars.len();
        if len < 2 {
            return None;
        }

        let mut best: Vec<Option<ResegmentPath>> = vec![None; len + 1];
        best[0] = Some(ResegmentPath::start());
        for start in 0..len {
            let Some(start_path) = best[start].clone() else {
                continue;
            };

            for end in start + 1..=len {
                let part = chars[start..end].iter().collect::<String>();
                if let Some(token) = domain_token_for_surface(&part) {
                    let next = start_path.extend(
                        start,
                        token,
                        self.frequency.cost(&part) + TOKEN_PENALTY,
                        part.chars().count(),
                    );
                    update_resegment_path(&mut best[end], next);
                }
                let Some(entries) = self.entries_for(&part) else {
                    continue;
                };
                let token = Token {
                    surface: part.clone(),
                    dictionary_form: part.clone(),
                    reasons: Vec::new(),
                    entries: ranked_entries(entries, LinderaPos::Other),
                    source_pos: None,
                    note_override: None,
                };
                let next = start_path.extend(
                    start,
                    token,
                    self.frequency.cost(&part) + TOKEN_PENALTY,
                    part.chars().count(),
                );
                update_resegment_path(&mut best[end], next);
            }

            if is_resegment_separator_char(chars[start]) {
                let part = chars[start].to_string();
                let next = start_path.extend(start, unknown_surface_token(part), TOKEN_PENALTY, 0);
                update_resegment_path(&mut best[start + 1], next);
            }

            let unknown_end = unknown_group_end(&chars, start);
            if unknown_end > start {
                let part = chars[start..unknown_end].iter().collect::<String>();
                let next = start_path.extend(
                    start,
                    Token {
                        surface: part.clone(),
                        dictionary_form: part,
                        reasons: Vec::new(),
                        entries: Vec::new(),
                        source_pos: None,
                        note_override: None,
                    },
                    UNKNOWN_RESEGMENT_COST,
                    0,
                );
                update_resegment_path(&mut best[unknown_end], next);
            }

            let part = chars[start].to_string();
            let next = start_path.extend(
                start,
                unknown_surface_token(part),
                UNKNOWN_RESEGMENT_COST,
                0,
            );
            update_resegment_path(&mut best[start + 1], next);
        }

        let mut path = best[len].clone()?;
        let mut tokens = Vec::new();
        while let Some(step) = path.step {
            tokens.push(step.token);
            path = best[step.previous]
                .clone()
                .expect("resegment path is linked");
        }
        tokens.reverse();
        Some(tokens)
    }

    fn split_ui_count_label_terms(&self, tokens: Vec<Token>) -> Vec<Token> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut index = 0usize;
        while index < tokens.len() {
            let token = &tokens[index];
            if tokens
                .get(index + 1)
                .is_some_and(|next| next.surface == "素材")
                && let Some(parts) = self.split_exp_material_prefix(token)
            {
                out.extend(parts);
                index += 1;
                continue;
            }
            if let Some(parts) = self.split_exact_known_compound(token, "所持数", &["所持", "数"])
            {
                out.extend(parts);
            } else if let Some(parts) =
                self.split_exact_known_compound(token, "合成数", &["合成", "数"])
            {
                out.extend(parts);
            } else {
                out.push(token.clone());
            }
            index += 1;
        }
        out
    }

    fn split_exp_material_prefix(&self, token: &Token) -> Option<Vec<Token>> {
        let prefix = match token.surface.as_str() {
            "武器EXP" => "武器",
            "共鳴者EXP" => "共鳴者",
            _ => return None,
        };
        Some(vec![
            self.exact_known_token(prefix)
                .unwrap_or_else(|| unknown_surface_token(prefix.to_string())),
            unknown_surface_token("EXP".to_string()),
        ])
    }

    fn split_exact_known_compound(
        &self,
        token: &Token,
        surface: &str,
        parts: &[&str],
    ) -> Option<Vec<Token>> {
        if token.surface != surface {
            return None;
        }
        parts
            .iter()
            .map(|part| self.exact_known_token(part))
            .collect()
    }

    fn exact_known_token(&self, surface: &str) -> Option<Token> {
        if let Some(token) = domain_token_for_surface(surface) {
            return Some(token);
        }
        let entries = self.entries_for(surface)?;
        Some(Token {
            surface: surface.to_string(),
            dictionary_form: surface.to_string(),
            reasons: Vec::new(),
            entries: ranked_entries(entries, LinderaPos::Other),
            source_pos: None,
            note_override: None,
        })
    }
    fn post_process_tokens(&self, tokens: Vec<Token>) -> Vec<Token> {
        let tokens = merge_domain_terms(tokens);
        let tokens = merge_unknown_katakana_runs(tokens);
        let tokens = merge_japanese_month_day_tokens(tokens);
        let tokens = merge_compact_numeric_unknowns(tokens);
        let tokens = merge_numeric_unit_unknowns(tokens);
        let tokens = merge_honorific_prefix_tokens(tokens);
        let tokens = self.normalize_suru_te_form_tokens(tokens);
        let tokens = self.normalize_suru_past_form_tokens(tokens);
        let tokens = self.normalize_suru_passive_form_tokens(tokens);
        let tokens = self.normalize_te_iru_auxiliary_tokens(tokens);
        let tokens = normalize_policy_homographs(tokens);
        let tokens = normalize_dekiru_stem_tokens(tokens);
        let tokens = split_contextual_slash_numeric_unknowns(tokens);
        merge_repeated_punctuation_unknowns(tokens)
    }

    fn normalize_suru_te_form_tokens(&self, tokens: Vec<Token>) -> Vec<Token> {
        // See docs/eval-metadata.md: under the current token model, して after a
        // suru-capable nominal gets one stable learner-facing primary analysis
        // as the te-form of する. The surrounding nominal carries the concrete
        // lexical meaning; future UI can expose the finer し + て decomposition
        // as an alternate grammar note.
        let mut normalized = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            let after_suru_nominal = normalized.last().is_some_and(is_suru_capable_nominal_token);
            if after_suru_nominal
                && is_shite_particle_homograph(token)
                && let Some(suru) = self.suru_te_form_token()
            {
                normalized.push(suru);
                index += 1;
            } else if after_suru_nominal
                && is_suru_renyou_token(token)
                && tokens.get(index + 1).is_some_and(is_te_particle_token)
                && let Some(suru) = self.suru_te_form_token()
            {
                normalized.push(suru);
                index += 2;
            } else {
                normalized.push(token.clone());
                index += 1;
            }
        }
        normalized
    }

    fn suru_te_form_token(&self) -> Option<Token> {
        let entries =
            suru_light_verb_entries(ranked_entries(self.entries_for("する")?, LinderaPos::Verb));
        Some(Token {
            surface: "して".to_string(),
            dictionary_form: "する".to_string(),
            reasons: vec!["連用形".to_string()],
            entries,
            source_pos: None,
            note_override: None,
        })
    }

    fn normalize_suru_past_form_tokens(&self, tokens: Vec<Token>) -> Vec<Token> {
        let mut normalized = Vec::with_capacity(tokens.len());
        for token in tokens {
            if is_shita_noun_homograph(&token)
                && normalized
                    .last()
                    .is_some_and(|previous| is_suru_past_context(previous, &token))
                && let Some(suru) = self.suru_past_form_token()
            {
                normalized.push(suru);
            } else if is_shita_noun_homograph(&token)
                && normalized
                    .last()
                    .is_none_or(|previous| previous.surface != "の")
                && let Some(suru) = self.suru_past_form_token()
            {
                normalized.push(suru);
            } else {
                normalized.push(token);
            }
        }
        normalized
    }

    fn suru_past_form_token(&self) -> Option<Token> {
        let entries =
            suru_light_verb_entries(ranked_entries(self.entries_for("する")?, LinderaPos::Verb));
        Some(Token {
            surface: "した".to_string(),
            dictionary_form: "する".to_string(),
            reasons: vec!["過去".to_string()],
            entries,
            source_pos: None,
            note_override: None,
        })
    }

    fn normalize_suru_passive_form_tokens(&self, tokens: Vec<Token>) -> Vec<Token> {
        let mut normalized = Vec::with_capacity(tokens.len());
        for token in tokens {
            if normalized.last().is_some_and(is_suru_capable_nominal_token)
                && is_sareru_passive_token(&token)
            {
                normalized.push(suru_passive_auxiliary_token());
            } else {
                normalized.push(token);
            }
        }
        normalized
    }

    fn normalize_te_iru_auxiliary_tokens(&self, tokens: Vec<Token>) -> Vec<Token> {
        let mut normalized = Vec::with_capacity(tokens.len());
        for token in tokens {
            let previous = normalized.last();
            let after_te_form = previous.is_some_and(is_te_form_connector_token);
            let after_te_contracted_verb_stem =
                previous.is_some_and(is_te_contracted_verb_stem_token);
            if after_te_form && is_iru_existential_token(&token) {
                normalized.push(te_iru_auxiliary_token());
            } else if (after_te_form || after_te_contracted_verb_stem)
                && is_teru_shine_homograph(&token)
            {
                normalized.push(contracted_te_iru_auxiliary_token());
            } else if after_te_form && is_ita_noun_homograph(&token) {
                normalized.push(te_iru_past_auxiliary_token());
            } else if after_te_form && is_iku_noun_homograph(&token) {
                normalized.push(te_iku_auxiliary_token());
            } else {
                normalized.push(token);
            }
        }
        normalized
    }
}

fn is_suru_capable_nominal_token(token: &Token) -> bool {
    token.entries.iter().any(|entry| {
        entry.senses.iter().any(|sense| {
            sense
                .part_of_speech
                .iter()
                .any(|pos| pos == "vs" || pos.starts_with("vs-"))
        })
    })
}

fn is_shite_particle_homograph(token: &Token) -> bool {
    token.surface == "して" && token.dictionary_form == "して"
}

fn is_suru_renyou_token(token: &Token) -> bool {
    token.surface == "し" && token.dictionary_form == "する"
}

fn is_suru_past_context(previous: &Token, token: &Token) -> bool {
    is_suru_capable_nominal_token(previous)
        || (token.surface == "した"
            && matches!(
                previous.surface.as_str(),
                "を" | "に" | "と" | "が" | "は" | "も" | "へ" | "で" | "から" | "まで"
            ))
}

fn is_sareru_passive_token(token: &Token) -> bool {
    token.surface == "される" && token.dictionary_form == "される"
}

fn is_te_particle_token(token: &Token) -> bool {
    token.surface == "て"
}

fn suru_light_verb_entries(mut entries: Vec<Entry>) -> Vec<Entry> {
    if let Some(entry) = entries.first_mut() {
        entry.popup_override = Some(PopupOverride {
            ruby: vec![RubySegment {
                text: "する".to_string(),
                furigana: None,
            }],
            glosses: vec!["to do; to carry out; to perform  (vs-i)".to_string()],
        });
    }
    entries
}

fn is_shita_noun_homograph(token: &Token) -> bool {
    token.surface == "した"
        && token.dictionary_form == "した"
        && token.entries.iter().any(|entry| {
            entry
                .senses
                .iter()
                .any(|sense| sense.part_of_speech.iter().any(|pos| pos == "n"))
        })
}

fn is_te_form_connector_token(token: &Token) -> bool {
    token.surface == "して"
        || token.surface == "て"
        || (token.surface == "で"
            && token
                .source_pos
                .is_some_and(|source_pos| source_pos == LinderaPos::Verb))
}

fn is_te_contracted_verb_stem_token(token: &Token) -> bool {
    token.reasons.iter().any(|reason| reason == "連用タ接続")
        && (token
            .source_pos
            .is_some_and(|source_pos| source_pos == LinderaPos::Verb)
            || token.entries.iter().any(|entry| {
                entry.senses.iter().any(|sense| {
                    sense
                        .part_of_speech
                        .iter()
                        .any(|pos| PosClass::of(pos) == PosClass::Verb)
                })
            }))
}

fn is_iru_existential_token(token: &Token) -> bool {
    token.surface == "いる" && token.dictionary_form == "いる"
}

fn is_ita_noun_homograph(token: &Token) -> bool {
    token.surface == "いた"
        && token.dictionary_form == "いた"
        && token.entries.iter().any(|entry| {
            entry
                .senses
                .iter()
                .any(|sense| sense.part_of_speech.iter().any(|pos| pos == "n"))
        })
}

fn is_teru_shine_homograph(token: &Token) -> bool {
    token.surface == "てる" && token.dictionary_form == "てる"
}

fn is_iku_noun_homograph(token: &Token) -> bool {
    token.surface == "いく"
        && token.entries.iter().any(|entry| {
            entry
                .senses
                .iter()
                .any(|sense| sense.part_of_speech.iter().any(|pos| pos == "n"))
        })
}

fn suru_passive_auxiliary_token() -> Token {
    Token {
        surface: "される".to_string(),
        dictionary_form: "される".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["される".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-v".to_string(), "v1".to_string()],
                glosses: vec!["to be ...-ed; passive of する".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "される".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be ...-ed; passive of する  (aux-v, v1)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: Some("Passive auxiliary after a suru-capable noun.".to_string()),
    }
}

fn te_iru_auxiliary_token() -> Token {
    Token {
        surface: "いる".to_string(),
        dictionary_form: "いる".to_string(),
        reasons: vec!["補助動詞".to_string()],
        entries: vec![Entry {
            kanji: vec!["居る".to_string()],
            kana: vec!["いる".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-v".to_string(), "v1".to_string()],
                glosses: vec!["to be ...-ing".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "いる".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be ...-ing  (aux-v, v1)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: None,
    }
}

fn contracted_te_iru_auxiliary_token() -> Token {
    Token {
        surface: "てる".to_string(),
        dictionary_form: "ている".to_string(),
        reasons: vec!["口語短縮".to_string()],
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["ている".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-v".to_string()],
                glosses: vec!["to be ...-ing; to have ...-ed".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ている".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be ...-ing; to have ...-ed  (aux-v)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: Some("てる · 口語短縮".to_string()),
    }
}

fn te_iru_past_auxiliary_token() -> Token {
    let mut token = te_iru_auxiliary_token();
    token.surface = "いた".to_string();
    token.reasons = vec!["補助動詞".to_string(), "過去".to_string()];
    token.note_override = Some("Past auxiliary in a ている chain.".to_string());
    token
}

fn te_iku_auxiliary_token() -> Token {
    Token {
        surface: "いく".to_string(),
        dictionary_form: "いく".to_string(),
        reasons: vec!["補助動詞".to_string()],
        entries: vec![Entry {
            kanji: vec!["行く".to_string()],
            kana: vec!["いく".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["v5k-s".to_string(), "vi".to_string()],
                glosses: vec!["to continue; to go on".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "いく".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to continue; to go on  (v5k-s, vi)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Verb),
        note_override: Some("Auxiliary in a ていく chain, indicating progression.".to_string()),
    }
}

fn normalize_policy_homographs(tokens: Vec<Token>) -> Vec<Token> {
    // See docs/eval-metadata.md: these entries are the learner-facing matrix for
    // common kana/status homographs where the primary popup should be stable,
    // not whichever rare JMdict/Lindera exact match happened to win.
    tokens
        .iter()
        .enumerate()
        .map(|(index, token)| match token.surface.as_str() {
            surface if canonical_particle_gloss(surface).is_some() => {
                canonical_particle_token(surface)
            }
            "て" => connective_te_token(),
            "た" if is_auxiliary_token(token) => past_ta_auxiliary_token(),
            "中" if is_status_chuu_suffix_context(previous_token(&tokens, index), token) => {
                status_chuu_suffix_token()
            }
            "日" if is_day_counter_context(previous_token(&tokens, index)) => day_counter_token(),
            "人" if is_person_counter_context(previous_token(&tokens, index)) => {
                person_counter_token()
            }
            "名" if is_person_counter_context(previous_token(&tokens, index)) => {
                mei_person_counter_token()
            }
            "数" | "必要" | "特定" | "終奏" => {
                domain_token_for_surface(&token.surface).unwrap_or_else(|| token.clone())
            }
            "〜" => nonlexical_symbol_token("〜"),
            "さん" if is_honorific_suffix_context(previous_token(&tokens, index)) => {
                honorific_suffix_token("さん", "Mr.; Mrs.; Miss; Ms.; -san")
            }
            "ちゃん" if is_honorific_suffix_context(previous_token(&tokens, index)) => {
                honorific_suffix_token("ちゃん", "suffix for familiar names, children, pets, etc.")
            }
            "なら" if is_nara_particle_context(previous_token(&tokens, index)) => {
                nara_conditional_particle_token()
            }
            "非" if is_bound_prefix_context(next_token(&tokens, index)) => {
                bound_prefix_token("非", "non-; un-; anti-", Some("ひ"))
            }
            "古" if is_bound_prefix_context(next_token(&tokens, index)) => {
                bound_prefix_token("古", "old; ancient", Some("こ"))
            }
            "んだ" => nda_explanatory_token(),
            "切替" => kirikae_token(),
            "ください" => kudasai_request_auxiliary_token(),
            "こと" => koto_nominalizer_token(),
            "する" => suru_primary_token(),
            "です" => desu_copula_token(),
            "この" => demonstrative_token("この", "this"),
            "その" => demonstrative_token("その", "that; the"),
            "あの" => demonstrative_token("あの", "that (over there)"),
            "ただ" => tada_adverb_token(),
            "ない" => nai_negative_token(),
            "なく" if token.dictionary_form == "ない" => nai_continuative_token(),
            "くる" => kuru_come_token("くる", Vec::new(), None),
            "きた" if is_kita_come_context(previous_token(&tokens, index), token) => {
                kuru_come_token(
                    "きた",
                    vec!["過去".to_string()],
                    Some("Past form of 来る in a て/でくる chain.".to_string()),
                )
            }
            "済" => status_sumi_token("済"),
            "済み" => status_sumi_token("済み"),
            "ねぇ" => elongated_particle_token("ねぇ", "ね"),
            "もの" => mono_thing_token(),
            "いい" => ii_adjective_token(),
            "いた" if token.dictionary_form == "いた" => iru_past_verb_token(),
            "なし" if token.dictionary_form == "ない" || token.dictionary_form == "無し" => {
                nashi_nominal_token()
            }
            "おり" if token.dictionary_form == "おる" => oru_continuative_token(token.clone()),
            "なき" if token.dictionary_form == "ない" || token.dictionary_form == "無き" => {
                literary_naki_token()
            }
            _ => token.clone(),
        })
        .collect()
}

fn previous_token(tokens: &[Token], index: usize) -> Option<&Token> {
    index.checked_sub(1).and_then(|index| tokens.get(index))
}

fn next_token(tokens: &[Token], index: usize) -> Option<&Token> {
    tokens.get(index + 1)
}

fn canonical_particle_gloss(surface: &str) -> Option<&'static str> {
    match surface {
        "の" => Some("possessive; of; 's  (prt)"),
        "に" => Some("to; at; in (target/location)  (prt)"),
        "を" => Some("direct object marker  (prt)"),
        "は" => Some("topic marker  (prt)"),
        "が" => Some("subject marker  (prt)"),
        "で" => Some("at; by; with (means/place)  (prt)"),
        "へ" => Some("to; toward (direction)  (prt)"),
        "から" => Some("from; because  (prt)"),
        "も" => Some("also; too  (prt)"),
        "と" => Some("and; with; quotation marker  (prt)"),
        _ => None,
    }
}

fn canonical_particle_token(surface: &str) -> Token {
    let gloss = canonical_particle_gloss(surface).expect("known canonical particle");
    Token {
        surface: surface.to_string(),
        dictionary_form: surface.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec![surface.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["prt".to_string()],
                glosses: vec![gloss.trim_end_matches("  (prt)").to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: surface.to_string(),
                    furigana: None,
                }],
                glosses: vec![gloss.to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Particle),
        note_override: None,
    }
}

fn connective_te_token() -> Token {
    Token {
        surface: "て".to_string(),
        dictionary_form: "て".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["て".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["prt".to_string()],
                glosses: vec!["and; then; -ing (connective)".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "て".to_string(),
                    furigana: None,
                }],
                glosses: vec!["and; then; -ing (connective)  (prt)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Particle),
        note_override: None,
    }
}

fn past_ta_auxiliary_token() -> Token {
    Token {
        surface: "た".to_string(),
        dictionary_form: "た".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["た".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-v".to_string()],
                glosses: vec!["past tense auxiliary".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "た".to_string(),
                    furigana: None,
                }],
                glosses: vec!["past tense auxiliary  (aux-v)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: None,
    }
}

fn is_auxiliary_token(token: &Token) -> bool {
    token.entries.iter().any(|entry| {
        entry.senses.iter().any(|sense| {
            sense
                .part_of_speech
                .iter()
                .any(|pos| pos == "aux-v" || pos == "aux-adj")
        })
    })
}

fn is_status_chuu_suffix_context(previous: Option<&Token>, token: &Token) -> bool {
    token.dictionary_form == "中"
        && previous.is_some_and(|previous| {
            previous.surface != "の"
                && previous.is_known()
                && previous.entries.iter().any(|entry| {
                    entry.senses.iter().any(|sense| {
                        sense.part_of_speech.iter().any(|pos| {
                            matches!(
                                PosClass::of(pos),
                                PosClass::Noun | PosClass::Expression | PosClass::Adjective
                            )
                        })
                    })
                })
        })
}

fn status_chuu_suffix_token() -> Token {
    Token {
        surface: "中".to_string(),
        dictionary_form: "中".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["中".to_string()],
            kana: vec!["ちゅう".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string(), "suf".to_string()],
                glosses: vec!["during; while; in the middle of".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "中".to_string(),
                    furigana: Some("ちゅう".to_string()),
                }],
                glosses: vec!["during; while; in the middle of  (n, suf)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn is_day_counter_context(previous: Option<&Token>) -> bool {
    previous.is_some_and(|previous| {
        previous
            .surface
            .chars()
            .next_back()
            .is_some_and(is_japanese_or_ascii_numeral)
    })
}

fn is_person_counter_context(previous: Option<&Token>) -> bool {
    previous.is_some_and(|previous| {
        previous
            .surface
            .chars()
            .next_back()
            .is_some_and(is_japanese_or_ascii_numeral)
    })
}

fn is_japanese_or_ascii_numeral(ch: char) -> bool {
    ch.is_ascii_digit()
        || matches!(
            ch,
            '〇' | '零'
                | '一'
                | '二'
                | '三'
                | '四'
                | '五'
                | '六'
                | '七'
                | '八'
                | '九'
                | '十'
                | '百'
                | '千'
                | '万'
        )
}

fn person_counter_token() -> Token {
    Token {
        surface: "人".to_string(),
        dictionary_form: "人".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["人".to_string()],
            kana: vec!["にん".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["ctr".to_string()],
                glosses: vec!["counter for people".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "人".to_string(),
                    furigana: Some("にん".to_string()),
                }],
                glosses: vec!["counter for people  (ctr)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn mei_person_counter_token() -> Token {
    Token {
        surface: "名".to_string(),
        dictionary_form: "名".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["名".to_string()],
            kana: vec!["めい".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["ctr".to_string()],
                glosses: vec!["counter for people".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "名".to_string(),
                    furigana: Some("めい".to_string()),
                }],
                glosses: vec!["counter for people  (ctr)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn day_counter_token() -> Token {
    Token {
        surface: "日".to_string(),
        dictionary_form: "日".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["日".to_string()],
            kana: vec!["にち".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string(), "ctr".to_string()],
                glosses: vec!["day(s); counter for days".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "日".to_string(),
                    furigana: Some("にち".to_string()),
                }],
                glosses: vec!["day(s); counter for days  (n, ctr)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn koto_nominalizer_token() -> Token {
    Token {
        surface: "こと".to_string(),
        dictionary_form: "こと".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["事".to_string()],
            kana: vec!["こと".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string()],
                glosses: vec!["thing; matter; act; fact; nominalizer".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "こと".to_string(),
                    furigana: None,
                }],
                glosses: vec!["thing; matter; act; fact; nominalizer  (n)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn suru_primary_token() -> Token {
    Token {
        surface: "する".to_string(),
        dictionary_form: "する".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["為る".to_string()],
            kana: vec!["する".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["vs-i".to_string()],
                glosses: vec!["to do; to carry out; to perform".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "する".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to do; to carry out; to perform  (vs-i)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Verb),
        note_override: None,
    }
}

fn desu_copula_token() -> Token {
    Token {
        surface: "です".to_string(),
        dictionary_form: "です".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["です".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["cop".to_string(), "aux-v".to_string()],
                glosses: vec!["to be; polite copula".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "です".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be; polite copula  (cop, aux-v)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: None,
    }
}

fn demonstrative_token(surface: &str, gloss: &str) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: surface.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec![surface.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["adj-pn".to_string()],
                glosses: vec![gloss.to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: surface.to_string(),
                    furigana: None,
                }],
                glosses: vec![format!("{gloss}  (adj-pn)")],
            }),
        }],
        source_pos: Some(LinderaPos::Adnominal),
        note_override: None,
    }
}

fn tada_adverb_token() -> Token {
    Token {
        surface: "ただ".to_string(),
        dictionary_form: "ただ".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["只".to_string()],
            kana: vec!["ただ".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["adv".to_string()],
                glosses: vec!["just; only; merely".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ただ".to_string(),
                    furigana: None,
                }],
                glosses: vec!["just; only; merely  (adv)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Adverb),
        note_override: None,
    }
}

fn nai_negative_token() -> Token {
    Token {
        surface: "ない".to_string(),
        dictionary_form: "ない".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["無い".to_string()],
            kana: vec!["ない".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["adj-i".to_string()],
                glosses: vec!["not; nonexistent; not being".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ない".to_string(),
                    furigana: None,
                }],
                glosses: vec!["not; nonexistent; not being  (adj-i)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Adjective),
        note_override: None,
    }
}

fn nai_continuative_token() -> Token {
    Token {
        surface: "なく".to_string(),
        dictionary_form: "ない".to_string(),
        reasons: vec!["連用テ接続".to_string()],
        entries: vec![Entry {
            kanji: vec!["無い".to_string()],
            kana: vec!["ない".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-adj".to_string()],
                glosses: vec!["not; non-; un-".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ない".to_string(),
                    furigana: None,
                }],
                glosses: vec!["not; non-; un-  (aux-adj)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: Some("なく · 連用テ接続".to_string()),
    }
}

fn nonlexical_symbol_token(surface: &str) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: surface.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec![surface.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["unc".to_string()],
                glosses: Vec::new(),
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: Vec::new(),
                glosses: Vec::new(),
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn is_honorific_suffix_context(previous: Option<&Token>) -> bool {
    previous.is_some_and(|previous| {
        previous
            .surface
            .chars()
            .any(|ch| is_kana(ch) || !ch.is_ascii())
            && !previous.surface.chars().all(is_quiet_lookup_separator)
    })
}

fn is_quiet_lookup_separator(ch: char) -> bool {
    matches!(
        ch,
        '、' | '。'
            | '・'
            | '：'
            | ':'
            | ','
            | '.'
            | '!'
            | '?'
            | '！'
            | '？'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '('
            | ')'
    )
}

fn honorific_suffix_token(surface: &str, gloss: &str) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: surface.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec![surface.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["suf".to_string()],
                glosses: vec![gloss.to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: surface.to_string(),
                    furigana: None,
                }],
                glosses: vec![format!("{gloss}  (suf)")],
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn is_nara_particle_context(previous: Option<&Token>) -> bool {
    previous.is_some_and(|previous| {
        if previous.surface.chars().all(is_quiet_lookup_separator) {
            return false;
        }
        if matches!(previous.surface.as_str(), "さん" | "ちゃん") {
            return true;
        }
        previous.entries.iter().any(|entry| {
            entry.senses.iter().any(|sense| {
                sense.part_of_speech.iter().any(|pos| {
                    matches!(
                        PosClass::of(pos),
                        PosClass::Noun | PosClass::Expression | PosClass::Adjective
                    ) || pos.as_str() == "pn"
                        || pos.as_str() == "suf"
                })
            })
        })
    })
}

fn nara_conditional_particle_token() -> Token {
    Token {
        surface: "なら".to_string(),
        dictionary_form: "なら".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["なら".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["prt".to_string()],
                glosses: vec!["if; in case of; as for".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "なら".to_string(),
                    furigana: None,
                }],
                glosses: vec!["if; in case of; as for  (prt)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Particle),
        note_override: None,
    }
}

fn is_bound_prefix_context(next: Option<&Token>) -> bool {
    next.is_some_and(|next| {
        !next.surface.chars().all(is_quiet_lookup_separator)
            && next.entries.iter().any(|entry| {
                entry.senses.iter().any(|sense| {
                    sense.part_of_speech.iter().any(|pos| {
                        matches!(
                            PosClass::of(pos),
                            PosClass::Noun | PosClass::Expression | PosClass::Adjective
                        ) || pos.as_str() == "suf"
                    })
                })
            })
    })
}

fn bound_prefix_token(surface: &str, gloss: &str, furigana: Option<&str>) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: surface.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec![surface.to_string()],
            kana: furigana
                .map(|reading| reading.to_string())
                .into_iter()
                .collect(),
            senses: vec![Sense {
                part_of_speech: vec!["pref".to_string()],
                glosses: vec![gloss.to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: surface.to_string(),
                    furigana: furigana.map(str::to_string),
                }],
                glosses: vec![format!("{gloss}  (pref)")],
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn nda_explanatory_token() -> Token {
    Token {
        surface: "んだ".to_string(),
        dictionary_form: "んだ".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec!["んだ".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["exp".to_string()],
                glosses: vec!["the fact is; it is that ...".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "んだ".to_string(),
                    furigana: None,
                }],
                glosses: vec!["the fact is; it is that ...  (exp)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Other),
        note_override: None,
    }
}

fn kirikae_token() -> Token {
    Token {
        surface: "切替".to_string(),
        dictionary_form: "切り替え".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["切り替え".to_string(), "切替".to_string()],
            kana: vec!["きりかえ".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string()],
                glosses: vec!["switching; exchange; changeover".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![
                    RubySegment {
                        text: "切".to_string(),
                        furigana: Some("き".to_string()),
                    },
                    RubySegment {
                        text: "替".to_string(),
                        furigana: Some("かえ".to_string()),
                    },
                ],
                glosses: vec!["switching; exchange; changeover  (n)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn is_kita_come_context(previous: Option<&Token>, token: &Token) -> bool {
    token.dictionary_form == "きた"
        && previous.is_some_and(|previous| {
            previous.surface.ends_with('て') || previous.surface.ends_with('で')
        })
}

fn kuru_come_token(surface: &str, reasons: Vec<String>, note: Option<String>) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: "来る".to_string(),
        reasons,
        entries: vec![Entry {
            kanji: vec!["来る".to_string()],
            kana: vec!["くる".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["vk".to_string(), "vi".to_string()],
                glosses: vec!["to come; to arrive; to come along".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: surface.to_string(),
                    furigana: None,
                }],
                glosses: vec!["to come; to arrive; to come along  (vk, vi)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Verb),
        note_override: note,
    }
}

fn kudasai_request_auxiliary_token() -> Token {
    Token {
        surface: "ください".to_string(),
        dictionary_form: "ください".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["下さい".to_string()],
            kana: vec!["ください".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["aux-v".to_string()],
                glosses: vec!["please (do)".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ください".to_string(),
                    furigana: None,
                }],
                glosses: vec!["please (do)  (aux-v)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::AuxVerb),
        note_override: Some("Polite request auxiliary.".to_string()),
    }
}

fn status_sumi_token(surface: &str) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: "済み".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["済み".to_string()],
            kana: vec!["すみ".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n-suf".to_string()],
                glosses: vec!["done; completed; settled".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![
                    RubySegment {
                        text: "済".to_string(),
                        furigana: Some("す".to_string()),
                    },
                    RubySegment {
                        text: "み".to_string(),
                        furigana: None,
                    },
                ],
                glosses: vec!["done; completed; settled  (n-suf)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: (surface == "済").then(|| "Clipped status suffix for 済み.".to_string()),
    }
}

fn elongated_particle_token(surface: &str, base: &str) -> Token {
    Token {
        surface: surface.to_string(),
        dictionary_form: base.to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: Vec::new(),
            kana: vec![base.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["prt".to_string()],
                glosses: vec!["sentence-ending particle".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: base.to_string(),
                    furigana: None,
                }],
                glosses: vec!["sentence-ending particle  (prt)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Particle),
        note_override: Some("Colloquial lengthening of ね.".to_string()),
    }
}

fn nashi_nominal_token() -> Token {
    Token {
        surface: "なし".to_string(),
        dictionary_form: "なし".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["無し".to_string()],
            kana: vec!["なし".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string(), "n-suf".to_string()],
                glosses: vec!["without; none".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "なし".to_string(),
                    furigana: None,
                }],
                glosses: vec!["without; none  (n, n-suf)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn mono_thing_token() -> Token {
    Token {
        surface: "もの".to_string(),
        dictionary_form: "もの".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["物".to_string()],
            kana: vec!["もの".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string()],
                glosses: vec!["thing; object; matter".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "もの".to_string(),
                    furigana: None,
                }],
                glosses: vec!["thing; object; matter  (n)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Noun),
        note_override: None,
    }
}

fn ii_adjective_token() -> Token {
    Token {
        surface: "いい".to_string(),
        dictionary_form: "いい".to_string(),
        reasons: Vec::new(),
        entries: vec![Entry {
            kanji: vec!["良い".to_string()],
            kana: vec!["いい".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["adj-i".to_string()],
                glosses: vec!["good; excellent; fine; nice; OK".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "いい".to_string(),
                    furigana: None,
                }],
                glosses: vec!["good; excellent; fine; nice; OK  (adj-i)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Adjective),
        note_override: None,
    }
}

fn iru_past_verb_token() -> Token {
    Token {
        surface: "いた".to_string(),
        dictionary_form: "いる".to_string(),
        reasons: vec!["過去".to_string()],
        entries: vec![Entry {
            kanji: vec!["居る".to_string()],
            kana: vec!["いる".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["v1".to_string(), "vi".to_string()],
                glosses: vec!["to be; to exist; to stay".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "いる".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be; to exist; to stay  (v1, vi)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Verb),
        note_override: None,
    }
}

fn oru_continuative_token(mut token: Token) -> Token {
    token.note_override = Some("Continuative form of おる.".to_string());
    token
}

fn literary_naki_token() -> Token {
    Token {
        surface: "なき".to_string(),
        dictionary_form: "ない".to_string(),
        reasons: vec!["連体形".to_string()],
        entries: vec![Entry {
            kanji: vec!["無い".to_string()],
            kana: vec!["ない".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["adj-i".to_string()],
                glosses: vec!["nonexistent; not being; without".to_string()],
                misc: vec!["uk".to_string()],
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "ない".to_string(),
                    furigana: None,
                }],
                glosses: vec!["nonexistent; not being; without  (adj-i)".to_string()],
            }),
        }],
        source_pos: Some(LinderaPos::Adjective),
        note_override: Some("Literary attributive なき, equivalent to のない.".to_string()),
    }
}

fn normalize_dekiru_stem_tokens(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| {
            if is_dekiru_stem_homograph(&token) {
                dekiru_stem_token(&token)
            } else {
                token
            }
        })
        .collect()
}

fn is_dekiru_stem_homograph(token: &Token) -> bool {
    token.surface == "でき"
        && token.dictionary_form == "できる"
        && token.entries.iter().any(|entry| {
            entry.senses.iter().any(|sense| {
                sense
                    .part_of_speech
                    .iter()
                    .any(|pos| pos == "v5r" || pos == "vt")
            })
        })
}

fn dekiru_stem_token(token: &Token) -> Token {
    Token {
        surface: token.surface.clone(),
        dictionary_form: "できる".to_string(),
        reasons: token.reasons.clone(),
        entries: vec![Entry {
            kanji: vec!["出来る".to_string()],
            kana: vec!["できる".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["v1".to_string(), "vi".to_string()],
                glosses: vec!["to be able to; can".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: Some(PopupOverride {
                ruby: vec![RubySegment {
                    text: "できる".to_string(),
                    furigana: None,
                }],
                glosses: vec!["to be able to; can  (v1, vi)".to_string()],
            }),
        }],
        source_pos: token.source_pos,
        note_override: token.note_override.clone(),
    }
}

fn merge_honorific_prefix_tokens(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if index + 1 < tokens.len()
            && is_honorific_prefix_token(&tokens[index])
            && let Some(token) = honorific_prefixed_token(&tokens[index], &tokens[index + 1])
        {
            merged.push(token);
            index += 2;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn is_honorific_prefix_token(token: &Token) -> bool {
    matches!(token.surface.as_str(), "ご" | "お")
}

fn honorific_prefixed_token(prefix: &Token, base: &Token) -> Option<Token> {
    if !base.is_known() {
        return None;
    }
    let mut entries = base.entries.clone();
    let entry = entries.first_mut()?;
    let base_ruby = entry
        .kana
        .first()
        .and_then(|reading| (reading != &base.dictionary_form).then(|| reading.clone()));
    entry.popup_override = Some(PopupOverride {
        ruby: vec![
            RubySegment {
                text: prefix.surface.clone(),
                furigana: None,
            },
            RubySegment {
                text: base.dictionary_form.clone(),
                furigana: base_ruby,
            },
        ],
        glosses: entry
            .senses
            .iter()
            .take(3)
            .map(format_sense_gloss)
            .collect(),
    });

    Some(Token {
        surface: format!("{}{}", prefix.surface, base.surface),
        dictionary_form: base.dictionary_form.clone(),
        reasons: base.reasons.clone(),
        entries,
        source_pos: base.source_pos,
        note_override: Some(format!(
            "Honorific {} prefix on {}.",
            prefix.surface, base.dictionary_form
        )),
    })
}

fn format_sense_gloss(sense: &Sense) -> String {
    let text = sense.glosses.join("; ");
    if sense.part_of_speech.is_empty() {
        text
    } else {
        format!("{text}  ({})", sense.part_of_speech.join(", "))
    }
}

fn merge_unknown_katakana_runs(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some(end) = unknown_katakana_run_end(&tokens, index) {
            let surface = tokens[index..end]
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>();
            merged.push(unknown_surface_token(surface));
            index = end;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn unknown_katakana_run_end(tokens: &[Token], start: usize) -> Option<usize> {
    if !is_katakana_surface_token(&tokens[start]) {
        return None;
    }

    let mut end = start + 1;
    let mut known_count = usize::from(tokens[start].is_known());
    let mut has_unknown = !tokens[start].is_known();
    while end < tokens.len() && is_katakana_surface_token(&tokens[end]) {
        known_count += usize::from(tokens[end].is_known());
        has_unknown |= !tokens[end].is_known();
        end += 1;
    }

    let run = &tokens[start..end];
    let surface_chars = run
        .iter()
        .map(|token| token.surface.chars().count())
        .sum::<usize>();
    let known_token_is_incidental = known_count == 0
        || (known_count == 1
            && run.first().is_some_and(|token| !token.is_known())
            && run.last().is_some_and(|token| !token.is_known()));
    (end > start + 1
        && has_unknown
        && known_token_is_incidental
        && surface_chars >= 2
        && !run.iter().any(|token| token.surface == "・"))
    .then_some(end)
}

fn is_katakana_surface_token(token: &Token) -> bool {
    !token.surface.is_empty() && token.surface.chars().all(is_katakana)
}

fn merge_japanese_month_day_tokens(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0usize;
    while index < tokens.len() {
        if index + 3 < tokens.len()
            && is_ascii_digit_surface(&tokens[index].surface)
            && tokens[index + 1].surface == "月"
            && is_ascii_digit_surface(&tokens[index + 2].surface)
            && tokens[index + 3].surface == "日"
        {
            let surface = tokens[index..index + 4]
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>();
            merged.push(unknown_surface_token(surface));
            index += 4;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn split_contextual_slash_numeric_unknowns(tokens: Vec<Token>) -> Vec<Token> {
    let mut split = Vec::with_capacity(tokens.len());
    for token in tokens {
        if should_split_slash_numeric_unknown(&token) && !is_purchase_count_ratio_context(&split) {
            push_slash_numeric_parts(&token.surface, &mut split);
        } else {
            split.push(token);
        }
    }
    split
}

fn is_purchase_count_ratio_context(previous: &[Token]) -> bool {
    previous.last().is_some_and(|token| token.surface == "：")
        && previous
            .get(previous.len().saturating_sub(2))
            .is_some_and(|token| token.surface == "購入可能数")
}

fn should_split_slash_numeric_unknown(token: &Token) -> bool {
    !token.is_known()
        && token.surface.contains('/')
        && !is_slash_date_surface(&token.surface)
        && token
            .surface
            .split('/')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_slash_date_surface(surface: &str) -> bool {
    let parts = surface.split('/').collect::<Vec<_>>();
    matches!(parts.as_slice(), [year, month, day]
        if year.len() == 4
            && month.len() == 2
            && day.len() == 2
            && year.chars().all(|ch| ch.is_ascii_digit())
            && month.chars().all(|ch| ch.is_ascii_digit())
            && day.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_ascii_digit_surface(surface: &str) -> bool {
    !surface.is_empty() && surface.chars().all(|ch| ch.is_ascii_digit())
}

fn push_slash_numeric_parts(surface: &str, out: &mut Vec<Token>) {
    let mut part = String::new();
    for ch in surface.chars() {
        if ch == '/' {
            if !part.is_empty() {
                out.push(unknown_surface_token(std::mem::take(&mut part)));
            }
            out.push(unknown_surface_token("/".to_string()));
        } else {
            part.push(ch);
        }
    }
    if !part.is_empty() {
        out.push(unknown_surface_token(part));
    }
}

fn cover_missing_unknown_tokens(line: &str, tokens: Vec<Token>) -> Vec<Token> {
    let chars = line.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return tokens;
    }
    if tokens.is_empty() {
        let mut covered = Vec::new();
        push_missing_unknowns(&chars, &mut covered);
        return covered;
    }

    let mut covered = Vec::with_capacity(tokens.len() * 2);
    let mut pos = 0usize;
    for token in tokens {
        let surface = token.surface.chars().collect::<Vec<_>>();
        if surface.is_empty() {
            continue;
        }
        let start = find_surface_from(&chars, &surface, pos).unwrap_or(pos.min(chars.len()));
        push_missing_unknowns(
            &chars[pos.min(chars.len())..start.min(chars.len())],
            &mut covered,
        );
        covered.push(token);
        pos = start.saturating_add(surface.len()).min(chars.len());
    }
    push_missing_unknowns(&chars[pos.min(chars.len())..], &mut covered);
    covered
}

fn find_surface_from(chars: &[char], surface: &[char], start: usize) -> Option<usize> {
    if surface.is_empty() || surface.len() > chars.len() {
        return None;
    }
    (start.min(chars.len())..=chars.len() - surface.len())
        .find(|&index| chars[index..index + surface.len()] == *surface)
}

fn push_missing_unknowns(chars: &[char], out: &mut Vec<Token>) {
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        let end = missing_unknown_run_end(chars, index);
        let surface = chars[index..end].iter().collect::<String>();
        out.push(unknown_surface_token(surface));
        index = end;
    }
}

fn missing_unknown_run_end(chars: &[char], start: usize) -> usize {
    if matches!(chars[start], '.' | '…') {
        let mut end = start + 1;
        while end < chars.len() && chars[end] == chars[start] {
            end += 1;
        }
        return end;
    }

    if chars[start].is_ascii_alphanumeric() {
        let mut end = start + 1;
        while end < chars.len() && chars[end].is_ascii_alphanumeric() {
            end += 1;
        }
        return end;
    }

    start + 1
}

fn merge_domain_terms(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if let Some((end, token)) = domain_term_at(&tokens, index) {
            merged.push(token);
            index = end;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn domain_term_at(tokens: &[Token], start: usize) -> Option<(usize, Token)> {
    let mut best: Option<(usize, Token)> = None;

    for spec in DOMAIN_KNOWN_TERMS {
        if let Some(end) = token_span_matches_surface(tokens, start, spec.surface) {
            update_domain_match(&mut best, end, domain_known_token(spec));
        }
    }

    for &surface in DOMAIN_UNKNOWN_TERMS {
        if let Some(end) = token_span_matches_surface(tokens, start, surface) {
            update_domain_match(&mut best, end, unknown_surface_token(surface.to_string()));
        }
    }

    best
}

fn domain_token_for_surface(surface: &str) -> Option<Token> {
    DOMAIN_KNOWN_TERMS
        .iter()
        .find(|spec| spec.surface == surface)
        .map(domain_known_token)
        .or_else(|| {
            is_domain_unknown_surface(surface).then(|| unknown_surface_token(surface.to_string()))
        })
}

fn is_domain_unknown_surface(surface: &str) -> bool {
    DOMAIN_UNKNOWN_TERMS.contains(&surface)
}

fn update_domain_match(best: &mut Option<(usize, Token)>, end: usize, token: Token) {
    let replace = best.as_ref().is_none_or(|(best_end, _)| end > *best_end);
    if replace {
        *best = Some((end, token));
    }
}

fn token_span_matches_surface(tokens: &[Token], start: usize, surface: &str) -> Option<usize> {
    let mut combined = String::new();
    for (offset, token) in tokens[start..].iter().enumerate() {
        combined.push_str(&token.surface);
        if combined == surface {
            return Some(start + offset + 1);
        }
        if !surface.starts_with(&combined) {
            return None;
        }
    }
    None
}

fn domain_known_token(spec: &KnownTermSpec) -> Token {
    Token {
        surface: spec.surface.to_string(),
        dictionary_form: spec.dictionary_form.to_string(),
        reasons: Vec::new(),
        entries: vec![domain_entry(spec)],
        source_pos: None,
        note_override: None,
    }
}

fn domain_entry(spec: &KnownTermSpec) -> Entry {
    Entry {
        kanji: vec![spec.dictionary_form.to_string()],
        kana: Vec::new(),
        senses: vec![Sense {
            part_of_speech: spec
                .part_of_speech
                .iter()
                .map(|pos| (*pos).to_string())
                .collect(),
            glosses: spec
                .glosses
                .iter()
                .map(|gloss| (*gloss).to_string())
                .collect(),
            misc: Vec::new(),
        }],
        common: true,
        popup_override: Some(PopupOverride {
            ruby: spec
                .ruby
                .iter()
                .map(|segment| RubySegment {
                    text: segment.text.to_string(),
                    furigana: segment.furigana.map(str::to_string),
                })
                .collect(),
            glosses: spec
                .glosses
                .iter()
                .map(|gloss| (*gloss).to_string())
                .collect(),
        }),
    }
}

fn merge_compact_numeric_unknowns(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if let Some(end) = compact_numeric_run_end(&tokens, index) {
            let surface = tokens[index..end]
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>();
            merged.push(unknown_surface_token(surface));
            index = end;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn merge_numeric_unit_unknowns(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if index + 1 < tokens.len()
            && is_numeric_value_unknown(&tokens[index])
            && is_numeric_unit_unknown(&tokens[index + 1])
            && should_merge_numeric_unit(&tokens, index)
        {
            let surface = format!("{}{}", tokens[index].surface, tokens[index + 1].surface);
            merged.push(unknown_surface_token(surface));
            index += 2;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn should_merge_numeric_unit(tokens: &[Token], index: usize) -> bool {
    let unit = tokens[index + 1].surface.as_str();
    if unit == "％"
        && tokens
            .get(index + 2)
            .is_some_and(|token| token.surface == "分")
    {
        return false;
    }
    if unit == "Pt" {
        if tokens
            .get(index + 2)
            .is_some_and(|token| token.surface == "につき")
        {
            return false;
        }
        if tokens
            .get(index + 2)
            .is_some_and(|token| token.surface == "に")
            && tokens
                .get(index + 3)
                .is_some_and(|token| token.surface == "つき")
        {
            return false;
        }
        if tokens.get(index + 2).is_none() && index > 0 && tokens[index - 1].surface == "を" {
            return false;
        }
    }
    true
}

fn is_numeric_value_unknown(token: &Token) -> bool {
    !token.is_known() && is_numeric_value_surface(&token.surface)
}

fn is_numeric_value_surface(surface: &str) -> bool {
    if surface.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }
    let mut parts = surface.split('.');
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or_default();
    parts.next().is_none()
        && !first.is_empty()
        && !second.is_empty()
        && first.chars().all(|ch| ch.is_ascii_digit())
        && second.chars().all(|ch| ch.is_ascii_digit())
}

fn is_numeric_unit_unknown(token: &Token) -> bool {
    !token.is_known() && matches!(token.surface.as_str(), "Pt" | "％")
}

fn compact_numeric_run_end(tokens: &[Token], start: usize) -> Option<usize> {
    if tokens[start].is_known() {
        return None;
    }
    let mut text = String::new();
    let mut best = None;
    for (offset, token) in tokens[start..].iter().enumerate() {
        if token.is_known() || !token.surface.chars().all(is_numeric_run_char) {
            break;
        }
        text.push_str(&token.surface);
        if !is_numeric_run_prefix(&text) {
            break;
        }
        if is_compact_numeric_unknown(&text) {
            best = Some(start + offset + 1);
        }
    }
    best.filter(|&end| end > start + 1)
}

fn is_numeric_run_char(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '.' | '/' | 'x' | 'X' | '*')
}

fn is_numeric_run_prefix(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, 'x' | 'X' | '*'))
}

fn is_compact_numeric_unknown(text: &str) -> bool {
    if matches!(text.chars().next(), Some('x' | 'X' | '*')) {
        let rest = &text[1..];
        return !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit());
    }
    if text.contains('/') {
        return text
            .split('/')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()));
    }
    if text.contains('.') {
        let mut parts = text.split('.');
        let first = parts.next().unwrap_or_default();
        let second = parts.next().unwrap_or_default();
        return parts.next().is_none()
            && !first.is_empty()
            && !second.is_empty()
            && first.chars().all(|ch| ch.is_ascii_digit())
            && second.chars().all(|ch| ch.is_ascii_digit());
    }
    false
}

fn merge_repeated_punctuation_unknowns(tokens: Vec<Token>) -> Vec<Token> {
    let mut merged = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if let Some(end) = repeated_punctuation_run_end(&tokens, index) {
            let surface = tokens[index..end]
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>();
            merged.push(unknown_surface_token(surface));
            index = end;
        } else {
            merged.push(tokens[index].clone());
            index += 1;
        }
    }
    merged
}

fn repeated_punctuation_run_end(tokens: &[Token], start: usize) -> Option<usize> {
    if tokens[start].is_known() || !matches!(tokens[start].surface.as_str(), "." | "…") {
        return None;
    }
    let punct = tokens[start].surface.as_str();
    let mut end = start + 1;
    while end < tokens.len() && !tokens[end].is_known() && tokens[end].surface == punct {
        end += 1;
    }
    (end > start + 1).then_some(end)
}

fn split_unknown_boundary_separator_tokens(tokens: Vec<Token>) -> Vec<Token> {
    let mut split = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token.is_known() || !has_boundary_separator(&token.surface) {
            split.push(token);
            continue;
        }
        split_boundary_separator_surface(&token.surface, &mut split);
    }
    split
}

fn has_boundary_separator(surface: &str) -> bool {
    surface
        .chars()
        .next()
        .is_some_and(is_resegment_separator_char)
        || surface
            .chars()
            .next_back()
            .is_some_and(is_resegment_separator_char)
}

fn split_boundary_separator_surface(surface: &str, out: &mut Vec<Token>) {
    let mut text = String::new();
    for ch in surface.chars() {
        if is_resegment_separator_char(ch) {
            if !text.is_empty() {
                out.push(unknown_surface_token(std::mem::take(&mut text)));
            }
            out.push(unknown_surface_token(ch.to_string()));
        } else {
            text.push(ch);
        }
    }
    if !text.is_empty() {
        out.push(unknown_surface_token(text));
    }
}

fn unknown_surface_token(surface: String) -> Token {
    let source_pos = source_pos_for_unknown_surface(&surface);
    Token {
        surface: surface.clone(),
        dictionary_form: surface,
        reasons: Vec::new(),
        entries: Vec::new(),
        source_pos,
        note_override: None,
    }
}

fn source_pos_for_unknown_surface(surface: &str) -> Option<LinderaPos> {
    is_ascii_abbreviation_surface(surface).then_some(LinderaPos::Other)
}

fn is_ascii_abbreviation_surface(surface: &str) -> bool {
    !surface.is_empty()
        && surface.chars().any(|ch| ch.is_ascii_alphabetic())
        && surface.chars().all(|ch| ch.is_ascii_alphanumeric())
        && surface
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .all(|ch| ch.is_ascii_uppercase())
}

#[derive(Debug, Clone)]
struct ResegmentPath {
    cost: f32,
    known_chars: usize,
    step: Option<ResegmentStep>,
}

#[derive(Debug, Clone)]
struct ResegmentStep {
    previous: usize,
    token: Token,
}

impl ResegmentPath {
    fn start() -> Self {
        Self {
            cost: 0.0,
            known_chars: 0,
            step: None,
        }
    }

    fn extend(&self, previous: usize, token: Token, cost: f32, known_chars: usize) -> Self {
        Self {
            cost: self.cost + cost,
            known_chars: self.known_chars + known_chars,
            step: Some(ResegmentStep { previous, token }),
        }
    }
}

fn update_resegment_path(slot: &mut Option<ResegmentPath>, candidate: ResegmentPath) {
    let replace = slot.as_ref().is_none_or(|current| {
        candidate.cost < current.cost
            || (candidate.cost == current.cost && candidate.known_chars > current.known_chars)
    });
    if replace {
        *slot = Some(candidate);
    }
}

fn should_resegment_unknown_surface(surface: &str) -> bool {
    surface.chars().any(is_resegment_separator_char)
        || (surface.chars().count() >= 4 && surface.chars().any(is_katakana))
}

fn should_accept_resegmented_unknown(surface: &str, tokens: &[Token]) -> bool {
    if tokens.len() <= 1 {
        return false;
    }
    if surface.contains('・')
        && !tokens
            .iter()
            .any(|token| is_domain_unknown_surface(&token.surface))
        && !has_boundary_separator_split(surface, tokens)
    {
        return false;
    }
    if has_boundary_separator_split(surface, tokens) {
        return true;
    }
    let resolved_count = tokens
        .iter()
        .filter(|token| is_resolved_resegment_token(token))
        .count();
    let resolved_chars = tokens
        .iter()
        .filter(|token| is_resolved_resegment_token(token))
        .map(|token| token.surface.chars().count())
        .sum::<usize>();
    let has_separator = tokens
        .iter()
        .any(|token| is_resegment_separator_surface(&token.surface));
    (resolved_count >= 2 || (resolved_count >= 1 && has_separator))
        && resolved_chars * 2 >= surface.chars().count()
}

fn has_boundary_separator_split(surface: &str, tokens: &[Token]) -> bool {
    let starts_with_separator = surface
        .chars()
        .next()
        .is_some_and(is_resegment_separator_char);
    let ends_with_separator = surface
        .chars()
        .next_back()
        .is_some_and(is_resegment_separator_char);
    (starts_with_separator
        && tokens
            .first()
            .is_some_and(|token| is_resegment_separator_surface(&token.surface)))
        || (ends_with_separator
            && tokens
                .last()
                .is_some_and(|token| is_resegment_separator_surface(&token.surface)))
}

fn is_resolved_resegment_token(token: &Token) -> bool {
    token.is_known() || is_domain_unknown_surface(&token.surface)
}

fn is_resegment_separator_surface(surface: &str) -> bool {
    let mut chars = surface.chars();
    matches!(chars.next(), Some(ch) if chars.next().is_none() && is_resegment_separator_char(ch))
}

fn is_resegment_separator_char(ch: char) -> bool {
    matches!(
        ch,
        '・' | '：' | ':' | '【' | '】' | '「' | '」' | '（' | '）' | '(' | ')'
    )
}

fn unknown_group_end(chars: &[char], start: usize) -> usize {
    if !is_unknown_group_char(chars[start]) {
        return start;
    }
    let mut end = start + 1;
    while end < chars.len() && is_unknown_group_char(chars[end]) {
        end += 1;
    }
    end
}

fn is_unknown_group_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '.' | '/' | '%' | '％' | '-' | '+' | '*' | '×' | '[' | ']'
        )
}

/// A span resolved against JMdict: the headword form that matched, its entries,
/// and whether the match was on the deinflected lemma (vs. the literal surface).
struct Resolution<'a> {
    form: String,
    entries: Vec<&'a Entry>,
    matched_lemma: bool,
    reasons: Vec<String>,
}

fn should_suppress_single_kana_content_lookup(
    slice: &[Morpheme],
    surface: &str,
    resolution: &Resolution<'_>,
) -> bool {
    let [morpheme] = slice else {
        return false;
    };
    let mut chars = surface.chars();
    let Some(ch) = chars.next() else {
        return false;
    };
    if chars.next().is_some() || !is_kana(ch) {
        return false;
    }
    if surface == "る"
        && !resolution
            .entries
            .iter()
            .any(|entry| entry_has_grammar_pos(entry))
    {
        return true;
    }
    if resolution
        .entries
        .iter()
        .any(|entry| entry_matches_lindera_pos(entry, morpheme.major_pos()))
    {
        return false;
    }
    resolution.entries.iter().all(|entry| {
        entry.senses.iter().all(|sense| {
            sense.part_of_speech.iter().all(|pos| {
                matches!(
                    PosClass::of(pos),
                    PosClass::Noun | PosClass::Verb | PosClass::Adjective | PosClass::Adverb
                )
            })
        })
    })
}

fn entry_has_grammar_pos(entry: &Entry) -> bool {
    entry
        .senses
        .iter()
        .flat_map(|sense| sense.part_of_speech.iter())
        .any(|pos| matches!(PosClass::of(pos), PosClass::Particle | PosClass::Auxiliary))
}

/// Concatenated literal surfaces of a span.
fn span_surface(slice: &[Morpheme]) -> String {
    slice.iter().map(|m| m.surface.as_str()).collect()
}

/// A span's fused form with only the last morpheme deinflected to its base form
/// (e.g. 食べ + ました → 食べる, but the leading morphemes stay as surface).
fn span_lemma(slice: &[Morpheme]) -> String {
    let (last, head) = slice.split_last().expect("span is non-empty");
    head.iter()
        .map(|m| m.surface.as_str())
        .chain(std::iter::once(last.base_form.as_str()))
        .collect()
}

/// Whether recursive deinflection may resolve this span as one dictionary token.
/// Single morphemes are always eligible. Multi-morpheme spans must not absorb
/// grammatical boundary tokens; the eval and hover contract intentionally keeps
/// auxiliaries and particles addressable as their own tokens.
fn can_deinflect_span(slice: &[Morpheme]) -> bool {
    slice.len() == 1
        || slice.iter().skip(1).all(|morpheme| {
            !matches!(
                morpheme.major_pos(),
                LinderaPos::AuxVerb | LinderaPos::Particle | LinderaPos::Other
            )
        })
}

/// Whether a multi-morpheme span is a grammatical false-merge: it swallows a
/// particle (助詞) yet the fused entry is not contentful. A purely nominal match
/// (いて = 射手, n) is a coincidence of the て-form + 居る pattern, so we reject it
/// and let the lattice split い + て instead; real compounds (手に入れる = exp,v1;
/// 一緒に = adv) survive because they carry a verb/adjective/adverb/expression sense.
fn is_false_particle_merge(slice: &[Morpheme], entries: &[&Entry]) -> bool {
    slice.iter().any(is_particle_morpheme)
        && !entries.iter().any(|entry| has_compound_worthy_pos(entry))
}

/// Clone entries, ordering the sense that agrees with Lindera's major POS first
/// so grammatical morphemes lead with their particle/auxiliary sense (は 助詞 →
/// topic marker; う 助動詞 → volitional) rather than a frequent noun homograph.
fn ranked_entries(entries: Vec<&Entry>, major: LinderaPos) -> Vec<Entry> {
    let mut entries: Vec<Entry> = entries.into_iter().cloned().collect();
    entries.sort_by_key(|entry| !entry_matches_lindera_pos(entry, major));
    entries
}

/// Load the frequency table from a `jpdb-freq/` directory beside the lexicon.
/// Missing/unreadable ⇒ an empty table (segmentation still works, but without
/// frequency disambiguation it degrades to a fewest-tokens preference).
fn load_sibling_frequency(lexicon: &Path) -> FrequencyTable {
    let Some(dir) = lexicon.parent().map(|parent| parent.join("jpdb-freq")) else {
        return FrequencyTable::empty();
    };
    if !dir.is_dir() {
        tracing::warn!(
            "no frequency dictionary at {}; segmentation quality reduced (install the JPDB freq dict)",
            dir.display()
        );
        return FrequencyTable::empty();
    }
    match FrequencyTable::load(&dir) {
        Ok(table) => {
            tracing::info!(
                "loaded {} frequency entries from {}",
                table.len(),
                dir.display()
            );
            table
        }
        Err(error) => {
            tracing::warn!(
                "failed to load frequency dictionary from {}: {error:#}",
                dir.display()
            );
            FrequencyTable::empty()
        }
    }
}

/// A token for a morpheme with no JMdict entry: reports the lemma as the
/// dictionary form but carries no entries (so `is_known()` is false).
fn unknown_token(morpheme: &Morpheme) -> Token {
    Token {
        surface: morpheme.surface.clone(),
        dictionary_form: morpheme.base_form.clone(),
        reasons: inflection_reasons(morpheme),
        entries: Vec::new(),
        source_pos: source_pos_for_unknown_surface(&morpheme.surface),
        note_override: None,
    }
}

/// Whether a morpheme is an IPADIC particle (助詞). Particles are word
/// boundaries, so they should not be absorbed into a content-word fusion.
fn is_particle_morpheme(morpheme: &Morpheme) -> bool {
    morpheme.major_pos() == LinderaPos::Particle
}

/// Whether a JMdict entry is "contentful" enough to justify fusing a span that
/// contains a particle: a verb, adjective, adverb, or set expression. Pure
/// nouns (and noun-only homographs) do not qualify.
fn has_compound_worthy_pos(entry: &Entry) -> bool {
    entry
        .senses
        .iter()
        .flat_map(|sense| sense.part_of_speech.iter())
        .any(|pos| PosClass::of(pos).is_compound_worthy())
}

/// Whether a JMdict entry has any sense whose part of speech agrees with a
/// Lindera (IPADIC) major part of speech. Used to surface the grammatically
/// correct sense of a homograph (は as the particle 助詞, not the noun 羽).
fn entry_matches_lindera_pos(entry: &Entry, major: LinderaPos) -> bool {
    entry
        .senses
        .iter()
        .flat_map(|sense| sense.part_of_speech.iter())
        .any(|pos| major.agrees_with_jmdict(pos))
}

/// A short note describing how an inflected surface relates to its base form,
/// derived from the IPADIC conjugation-form feature (e.g. 連用形). Empty for
/// uninflected morphemes.
fn inflection_reasons(morpheme: &Morpheme) -> Vec<String> {
    if morpheme.is_inflected() {
        morpheme.conjugation_form.clone().into_iter().collect()
    } else {
        Vec::new()
    }
}

/// A segmented token and its dictionary resolution (if any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Token {
    /// The exact recognized substring.
    pub surface: String,
    /// The resolved dictionary headword (== surface for an exact match).
    pub dictionary_form: String,
    /// Deinflection reasons applied, outermost first (empty for exact matches).
    pub reasons: Vec<String>,
    /// Matching dictionary entries (empty for unknown tokens).
    pub entries: Vec<Entry>,
    #[serde(skip)]
    pub source_pos: Option<LinderaPos>,
    #[serde(skip)]
    pub note_override: Option<String>,
}

impl Token {
    pub fn is_known(&self) -> bool {
        !self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kanji: &[&str], kana: &[&str], pos: &str, glosses: &[&str]) -> Entry {
        entry_with_common(kanji, kana, pos, glosses, true)
    }

    fn entry_with_common(
        kanji: &[&str],
        kana: &[&str],
        pos: &str,
        glosses: &[&str],
        common: bool,
    ) -> Entry {
        Entry {
            kanji: kanji.iter().map(|s| s.to_string()).collect(),
            kana: kana.iter().map(|s| s.to_string()).collect(),
            senses: vec![Sense {
                part_of_speech: vec![pos.to_string()],
                glosses: glosses.iter().map(|s| s.to_string()).collect(),
                misc: Vec::new(),
            }],
            common,
            popup_override: None,
        }
    }

    fn sample_dict() -> Dictionary {
        Dictionary::from_entries(vec![
            entry(&["食べる"], &["たべる"], "v1", &["to eat"]),
            entry(&["飲む"], &["のむ"], "v5m", &["to drink"]),
            entry(&["買う"], &["かう"], "v5u", &["to buy"]),
            entry(&["高い"], &["たかい"], "adj-i", &["high", "expensive"]),
            entry(&["水"], &["みず"], "n", &["water"]),
        ])
    }

    fn token(surface: &str, dictionary_form: &str, pos: &str) -> Token {
        Token {
            surface: surface.to_string(),
            dictionary_form: dictionary_form.to_string(),
            reasons: Vec::new(),
            entries: vec![entry(&[dictionary_form], &[surface], pos, &["test gloss"])],
            source_pos: None,
            note_override: None,
        }
    }

    /// Dictionary forms of the known tokens for a line, in order.
    fn known_forms(dict: &Dictionary, line: &str) -> Vec<String> {
        dict.analyze_line(line)
            .into_iter()
            .filter(Token::is_known)
            .map(|token| token.dictionary_form)
            .collect()
    }

    fn surfaces(dict: &Dictionary, line: &str) -> Vec<String> {
        dict.analyze_line(line)
            .into_iter()
            .map(|token| token.surface)
            .collect()
    }

    #[test]
    fn resolves_uninflected_noun() {
        let dict = sample_dict();
        let token = dict
            .analyze_line("水")
            .into_iter()
            .find(Token::is_known)
            .expect("known token");
        assert_eq!(token.surface, "水");
        assert_eq!(token.dictionary_form, "水");
        assert!(token.reasons.is_empty());
    }

    #[test]
    fn lemmatizes_inflected_verbs_and_adjectives() {
        // Lindera segments + lemmatizes; the lemma is looked up in JMdict.
        let dict = sample_dict();
        assert!(known_forms(&dict, "食べた").contains(&"食べる".to_string()));
        assert!(known_forms(&dict, "食べました").contains(&"食べる".to_string()));
        assert!(known_forms(&dict, "飲みます").contains(&"飲む".to_string()));
        assert!(known_forms(&dict, "買った").contains(&"買う".to_string()));
        assert!(known_forms(&dict, "高かった").contains(&"高い".to_string()));
    }

    #[test]
    fn resegments_long_unknown_katakana_compound_into_known_terms() {
        let dict = Dictionary::from_entries(vec![
            entry(&["クリティカル"], &[], "adj-na", &["critical"]),
            entry(&["ダメージ"], &[], "n", &["damage"]),
            entry(&["アップ"], &[], "n", &["increase"]),
            entry(&["フラク"], &[], "n", &["frac"]),
            entry(&["トシ"], &[], "n", &["toshi"]),
            entry(&["デ"], &[], "n", &["de"]),
            entry(&["ス"], &[], "n", &["su"]),
        ]);

        assert_eq!(
            surfaces(&dict, "クリティカルダメージアップ"),
            vec!["クリティカル", "ダメージ", "アップ"]
        );
    }

    #[test]
    fn keeps_unresolved_katakana_name_as_one_unknown() {
        let dict = Dictionary::from_entries(vec![entry(&["ダメージ"], &[], "n", &["damage"])]);
        let tokens = dict.analyze_line("ラハイロイ");

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].surface, "ラハイロイ");
        assert!(!tokens[0].is_known());
    }

    #[test]
    fn merges_unresolved_katakana_name_around_incidental_known_token() {
        let dict = Dictionary::from_entries(vec![entry(&["スター"], &[], "n", &["star"])]);
        let tokens = dict.analyze_line("ナスターシャ");

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].surface, "ナスターシャ");
        assert!(!tokens[0].is_known());
    }

    #[test]
    fn keeps_mostly_known_katakana_compound_split_with_ocr_tail() {
        let dict = Dictionary::from_entries(vec![
            entry(&["クリティカル"], &[], "adj-na", &["critical"]),
            entry(&["ダメージ"], &[], "n", &["damage"]),
        ]);

        assert_eq!(
            surfaces(&dict, "クリティカルダメージアツプ"),
            vec!["クリティカル", "ダメージ", "アツプ"]
        );
    }

    #[test]
    fn resegments_domain_unknown_with_leading_separator() {
        let dict = Dictionary::from_entries(vec![
            entry(&["ソラ"], &[], "n", &["sky"]),
            entry(&["大地"], &["だいち"], "n", &["earth"]),
        ]);

        assert_eq!(
            surfaces(&dict, "ソラの大地・ラハイロイ"),
            vec!["ソラ", "の", "大地", "・", "ラハイロイ"]
        );
    }

    #[test]
    fn resegments_domain_unknown_with_known_tail() {
        let dict = Dictionary::from_entries(vec![entry(&["エンド"], &[], "n", &["end"])]);

        assert_eq!(
            surfaces(&dict, "ラハイロイ・エンドボ"),
            vec!["ラハイロイ", "・", "エンド", "ボ"]
        );
    }

    #[test]
    fn keeps_middle_dot_proper_name_without_domain_anchor() {
        let dict = Dictionary::from_entries(vec![
            entry(&["ディ"], &[], "n", &["D"]),
            entry(&["マ"], &[], "n", &["ma"]),
            entry(&["プレーン"], &[], "n", &["plane"]),
            entry(&["ズ"], &[], "n", &["z"]),
        ]);

        assert_eq!(
            surfaces(&dict, "ディマー・プレーンズ"),
            vec!["ディマー・プレーンズ"]
        );
    }

    #[test]
    fn splits_boundary_separators_out_of_unknown_runs() {
        let dict = Dictionary::from_entries(vec![entry(&["マトリクス"], &[], "n", &["matrix"])]);

        assert_eq!(surfaces(&dict, "CV：伊藤美来")[..2], ["CV", "："]);
        assert_eq!(surfaces(&dict, "マトリクス・"), vec!["マトリクス", "・"]);
    }

    #[test]
    fn merges_domain_unknown_terms() {
        let dict = Dictionary::from_entries(vec![
            entry(&["音"], &["おと"], "n", &["sound"]),
            entry(&["骸"], &["むくろ"], "n", &["corpse"]),
            entry(&["潮音"], &["ちょうおん"], "n", &["sound of waves"]),
            entry(&["スキル"], &[], "n", &["skill"]),
        ]);
        let tokens = dict.analyze_line("音骸スキル");

        assert_eq!(surfaces(&dict, "音骸スキル"), vec!["音骸", "スキル"]);
        assert!(!tokens[0].is_known());
        assert!(tokens[1].is_known());

        let tide_sound = dict.analyze_line("潮音任務");
        assert_eq!(tide_sound[0].surface, "潮音");
        assert!(!tide_sound[0].is_known());
    }

    #[test]
    fn merges_wuthering_domain_proper_nouns() {
        let dict = Dictionary::from_entries(vec![
            entry(&["リンク"], &[], "n", &["link"]),
            entry(&["ドス"], &[], "n", &["DOS"]),
            entry(&["パイン"], &[], "n", &["pine"]),
            entry(&["ダメージ"], &[], "n", &["damage"]),
            entry(&["アップ"], &[], "n", &["increase"]),
            entry(&["スター"], &[], "n", &["star"]),
            entry(&["シ"], &[], "n", &["si"]),
            entry(&["グリフ"], &[], "n", &["glyph"]),
            entry(&["ェ"], &[], "prt", &["to"]),
            entry(&["クス"], &[], "n", &["camphor tree"]),
            entry(&["拾"], &["じゅう"], "num", &["ten"]),
            entry(&["方"], &["かた"], "n", &["direction"]),
            entry(&["薬局"], &["やっきょく"], "n", &["pharmacy"]),
            entry(&["で"], &[], "prt", &["at"]),
            entry(&["購入"], &["こうにゅう"], "n,vs,vt", &["purchase"]),
            entry(&["マウント"], &[], "n", &["mount"]),
            entry(&["ギャラ"], &[], "n", &["appearance fee"]),
            entry(&["無音"], &["むおん"], "n", &["silence"]),
            entry(&["連星"], &["れんせい"], "n", &["binary star"]),
            entry(&["任務"], &["にんむ"], "n", &["mission"]),
            entry(&["ブラ"], &[], "n", &["bra"]),
            entry(&["ン"], &[], "n-pref", &["some"]),
            entry(&["ト"], &[], "n", &["G"]),
            entry(&["焦熱"], &["しょうねつ"], "n", &["scorching heat"]),
            entry(&["レジ"], &[], "n", &["cash register"]),
            entry(&["イン"], &[], "pref", &["in"]),
            entry(&["イド"], &[], "n", &["id"]),
            entry(&["マター"], &[], "n", &["matter"]),
            entry(&["スペース"], &[], "n", &["space"]),
        ]);

        let linked_spine = dict.analyze_line("リンクドスパイン");
        assert_eq!(linked_spine.len(), 1);
        assert_eq!(linked_spine[0].surface, "リンクドスパイン");
        assert!(!linked_spine[0].is_known());

        assert_eq!(
            surfaces(&dict, "クールタイム：20秒"),
            vec!["クールタイム", "：", "20", "秒"]
        );
        assert_eq!(
            surfaces(&dict, "【共形エネルギー】"),
            vec!["【", "共形エネルギー", "】"]
        );
        assert_eq!(
            surfaces(&dict, "気動ダメージアップ"),
            vec!["気動", "ダメージ", "アップ"]
        );
        assert_eq!(
            surfaces(&dict, "ロスト・ドリーム"),
            vec!["ロスト・ドリーム"]
        );
        assert_eq!(surfaces(&dict, "イレーナ"), vec!["イレーナ"]);
        assert_eq!(surfaces(&dict, "ダーニャ"), vec!["ダーニャ"]);
        assert_eq!(surfaces(&dict, "フラクトシデス"), vec!["フラクトシデス"]);
        assert_eq!(surfaces(&dict, "ナスターシャ"), vec!["ナスターシャ"]);
        assert_eq!(surfaces(&dict, "グリフェックス"), vec!["グリフェックス"]);
        assert_eq!(
            surfaces(&dict, "拾方薬局で購入"),
            vec!["拾方薬局", "で", "購入"]
        );
        assert_eq!(
            surfaces(&dict, "マウントギャラルの無音"),
            vec!["マウントギャラル", "の", "無音"]
        );
        assert_eq!(
            surfaces(&dict, "連星任務・ブラント"),
            vec!["連星", "任務", "・", "ブラント"]
        );
        assert_eq!(surfaces(&dict, "焦熱レジ..."), vec!["焦熱", "レジ..."]);
        assert_eq!(surfaces(&dict, "気動イン..."), vec!["気動", "イン..."]);
        assert_eq!(
            surfaces(&dict, "ヴォイドマター粒"),
            vec!["ヴォイドマター粒"]
        );
        assert_eq!(surfaces(&dict, "オイドマター"), vec!["オイドマター"]);
        assert_eq!(
            surfaces(&dict, "ヴォイドスペースへ"),
            vec!["ヴォイドスペース", "へ"]
        );
    }

    #[test]
    fn keeps_ui_dates_as_single_value_tokens() {
        let dict = Dictionary::from_entries(vec![
            entry(&["月"], &["つき"], "n", &["moon"]),
            entry(&["日"], &["ひ"], "n", &["day"]),
        ]);

        assert_eq!(surfaces(&dict, "2月2日"), vec!["2月2日"]);
        assert_eq!(surfaces(&dict, "2026/05/22"), vec!["2026/05/22"]);
    }

    #[test]
    fn keeps_clipped_material_fragments_as_unknown_terms() {
        let dict = Dictionary::from_entries(vec![
            entry(&["声"], &["こえ"], "n", &["voice"]),
            entry(&["律"], &["りつ"], "n", &["law"]),
            entry(&["中音"], &["ちゅうおん"], "n", &["medium tone"]),
            entry(&["叫"], &["きょう"], "n", &["shout"]),
        ]);

        let tokens = dict.analyze_line("声律の苗 中音・叫...");

        assert_eq!(
            tokens
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<Vec<_>>(),
            vec!["声律", "の", "苗", "中音", "・", "叫..."]
        );
        assert!(!tokens[0].is_known());
        assert!(!tokens[5].is_known());
    }

    #[test]
    fn merges_domain_known_terms() {
        let dict = sample_dict();
        let tokens = dict.analyze_line("購入可能数");

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].dictionary_form, "購入可能数");
        assert!(tokens[0].is_known());
        assert_eq!(
            tokens[0].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["available purchase count; purchasable quantity  (n)"]
        );
    }

    #[test]
    fn splits_ui_count_label_compounds() {
        let dict = Dictionary::from_entries(vec![
            entry(&["所持"], &["しょじ"], "n,vs,vt", &["possession"]),
            entry(&["購入"], &["こうにゅう"], "n,vs,vt", &["purchase"]),
            entry(&["可能"], &["かのう"], "adj-na,n", &["possible"]),
            entry(&["合成"], &["ごうせい"], "n,vs,vt,adj-no", &["composition"]),
            entry(&["数"], &["すう"], "n", &["number"]),
            entry(&["武器"], &["ぶき"], "n", &["weapon"]),
            entry(&["素材"], &["そざい"], "n", &["material"]),
            entry(
                &["購入可能数"],
                &["こうにゅうかのうすう"],
                "n",
                &["available purchase count"],
            ),
            entry(&["合成数"], &["ごうせいすう"], "n", &["synthetic number"]),
        ]);

        assert_eq!(
            surfaces(&dict, "所持数：15"),
            vec!["所持", "数", "：", "15"]
        );
        assert_eq!(
            surfaces(&dict, "購入可能数：1/1"),
            vec!["購入可能数", "：", "1/1"]
        );
        assert_eq!(surfaces(&dict, "合成数 1"), vec!["合成", "数", "1"]);
        assert_eq!(surfaces(&dict, "武器EXP素材"), vec!["武器", "EXP", "素材"]);
        assert_eq!(surfaces(&dict, "武器EXP"), vec!["武器EXP"]);
    }

    #[test]
    fn resolves_domain_game_terms_to_preferred_popup_metadata() {
        let dict = sample_dict();
        let tokens = dict.analyze_line("売り切れショップ共鳴者秒間セットドロップ特級装備");

        assert_eq!(tokens.len(), 8);
        assert_eq!(tokens[0].surface, "売り切れ");
        assert_eq!(tokens[0].dictionary_form, "売り切れる");
        assert_eq!(
            tokens[0].entries[0].senses[0].part_of_speech,
            vec!["v1", "vi"]
        );
        assert_eq!(
            tokens[1].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["shop; store  (n)"]
        );
        assert_eq!(
            tokens[2].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["resonator (Wuthering Waves term)"]
        );
        assert_eq!(tokens[3].entries[0].senses[0].part_of_speech, vec!["n-suf"]);
        assert_eq!(
            tokens[3].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["for ... seconds; interval measured in seconds  (n-suf)"]
        );
        assert_eq!(
            tokens[4].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["set  (n)"]
        );
        assert_eq!(
            tokens[5].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["drop; loot drop  (game UI noun)"]
        );
        assert_eq!(
            tokens[6].entries[0].senses[0].part_of_speech,
            vec!["n", "adj-no"]
        );
        assert_eq!(
            tokens[7].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["equipment; outfit; to equip  (n, vs, vt)"]
        );
    }

    #[test]
    fn keeps_domain_currency_as_unknown_name() {
        let dict = Dictionary::from_entries(vec![
            entry(&["星"], &["ほし"], "n", &["star"]),
            entry(&["声"], &["こえ"], "n", &["voice"]),
        ]);

        let tokens = dict.analyze_line("星声");

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].surface, "星声");
        assert!(!tokens[0].is_known());
    }

    #[test]
    fn merges_compact_numeric_unknown_runs() {
        let dict = sample_dict();

        assert_eq!(surfaces(&dict, "0/20"), vec!["0", "/", "20"]);
        assert_eq!(
            surfaces(&dict, "進捗：1/1"),
            vec!["進捗", "：", "1", "/", "1"]
        );
        assert_eq!(surfaces(&dict, "2.40"), vec!["2.40"]);
        assert_eq!(surfaces(&dict, "100Pt"), vec!["100Pt"]);
        assert_eq!(surfaces(&dict, "7.2％"), vec!["7.2％"]);
        assert_eq!(surfaces(&dict, "400％分"), vec!["400", "％", "分"]);
        assert_eq!(surfaces(&dict, "1Ptにつき"), vec!["1", "Pt", "につき"]);
        assert_eq!(surfaces(&dict, "を10Pt"), vec!["を", "10", "Pt"]);
    }

    #[test]
    fn keeps_unknown_punctuation_tokens_covering_the_line() {
        let dict = Dictionary::from_entries(vec![entry(&["斉爆効果"], &[], "n", &["effect"])]);

        assert_eq!(
            surfaces(&dict, "【斉爆効果】"),
            vec!["【", "斉爆効果", "】"]
        );
        assert_eq!(surfaces(&dict, "◆"), vec!["◆"]);
        assert_eq!(surfaces(&dict, "……"), vec!["……"]);
    }

    #[test]
    fn inflected_form_carries_a_reason_note() {
        let dict = sample_dict();
        let verb = dict
            .analyze_line("食べた")
            .into_iter()
            .find(|token| token.dictionary_form == "食べる")
            .expect("verb token");
        assert_ne!(verb.surface, verb.dictionary_form);
        // The conjugation-form note (IPADIC 活用形) is surfaced as a reason.
        assert!(!verb.reasons.is_empty());
    }

    #[test]
    fn resolves_suru_after_noun() {
        let dict = Dictionary::from_entries(vec![
            entry(&["勉強"], &["べんきょう"], "n", &["study"]),
            // include kanji + kana orthographies of suru so the lemma resolves
            // regardless of which IPADIC writes for 原形.
            entry(&["為る", "する"], &["する"], "vs-i", &["to do"]),
        ]);
        let forms = known_forms(&dict, "勉強します");
        assert!(forms.contains(&"勉強".to_string()), "forms: {forms:?}");
        assert!(
            forms.iter().any(|f| f == "する" || f == "為る"),
            "forms: {forms:?}"
        );
    }

    #[test]
    fn suru_nominal_te_form_beats_shite_particle_homograph() {
        let dict = Dictionary::from_entries(vec![
            entry(&["入力"], &["にゅうりょく"], "vs", &["input"]),
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry_with_common(&[], &["して"], "prt", &["by means of"], true),
            entry(
                &["下さる"],
                &["くださる"],
                "v5aru",
                &["to kindly do for one"],
            ),
        ]);
        let tokens = dict.analyze_line("入力してください");
        let token = tokens
            .iter()
            .find(|token| token.surface == "して")
            .unwrap_or_else(|| panic!("して token in {tokens:?}"))
            .clone();
        assert_eq!(token.dictionary_form, "する");
        assert_eq!(token.reasons, vec!["連用形".to_string()]);
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["vs-i".to_string()]);
    }

    #[test]
    fn suru_nominal_past_form_beats_shita_noun_homograph() {
        let dict = Dictionary::from_entries(vec![
            entry(&["発動"], &["はつどう"], "vs", &["activation"]),
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry(&["下"], &["した"], "n", &["below"]),
        ]);
        let tokens = dict.analyze_line("発動した");
        let token = tokens
            .iter()
            .find(|token| token.surface == "した")
            .unwrap_or_else(|| panic!("した token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "する");
        assert_eq!(token.reasons, vec!["過去".to_string()]);
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["vs-i".to_string()]);
    }

    #[test]
    fn suru_past_after_case_particle_beats_shita_noun_homograph() {
        let dict = Dictionary::from_entries(vec![
            entry(&["前"], &["まえ"], "n", &["before"]),
            entry_with_common(&[], &["に"], "prt", &["indirect object marker"], true),
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry(&["下"], &["した"], "n", &["below"]),
        ]);
        let tokens = dict.analyze_line("前にした");
        let token = tokens
            .iter()
            .find(|token| token.surface == "した")
            .unwrap_or_else(|| panic!("した token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "する");
        assert_eq!(token.reasons, vec!["過去".to_string()]);
    }

    #[test]
    fn shita_after_no_stays_noun_for_location_phrase() {
        let dict = Dictionary::from_entries(vec![
            entry(&["机"], &["つくえ"], "n", &["desk"]),
            entry_with_common(&[], &["の"], "prt", &["possessive marker"], true),
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry(&["下"], &["した"], "n", &["below"]),
        ]);
        let tokens = dict.analyze_line("机のした");
        let token = tokens
            .iter()
            .find(|token| token.surface == "した")
            .unwrap_or_else(|| panic!("した token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "した");
    }

    #[test]
    fn line_initial_shita_prefers_suru_past_over_location_noun() {
        let dict = Dictionary::from_entries(vec![
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry(&["下"], &["した"], "n", &["below"]),
        ]);
        let tokens = dict.analyze_line("した後");
        let token = tokens
            .iter()
            .find(|token| token.surface == "した")
            .unwrap_or_else(|| panic!("した token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "する");
        assert_eq!(token.reasons, vec!["過去".to_string()]);
    }

    #[test]
    fn te_iru_becomes_progressive_auxiliary() {
        let dict = Dictionary::from_entries(vec![
            entry(&["見る"], &["みる"], "v1", &["to see"]),
            entry_with_common(&[], &["て"], "prt", &["connective te-form particle"], true),
            entry(&["いる"], &["いる"], "v1", &["to be"]),
        ]);
        let tokens = dict.analyze_line("見ている");
        let token = tokens
            .iter()
            .find(|token| token.surface == "いる")
            .unwrap_or_else(|| panic!("いる token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "いる");
        assert_eq!(token.reasons, vec!["補助動詞".to_string()]);
        assert_eq!(token.source_pos, Some(LinderaPos::AuxVerb));
        let popup = token.entries[0].popup_override.as_ref().expect("popup");
        assert_eq!(
            popup.glosses,
            vec!["to be ...-ing  (aux-v, v1)".to_string()]
        );
    }

    #[test]
    fn te_ita_becomes_past_progressive_auxiliary() {
        let dict = Dictionary::from_entries(vec![
            entry(&["見る"], &["みる"], "v1", &["to see"]),
            entry_with_common(&[], &["て"], "prt", &["connective te-form particle"], true),
            entry(&["いる"], &["いる"], "v1", &["to be"]),
            entry(&["板"], &["いた"], "n", &["board"]),
        ]);
        let tokens = dict.analyze_line("見ていた");
        let token = tokens
            .iter()
            .find(|token| token.surface == "いた")
            .unwrap_or_else(|| panic!("いた token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "いる");
        assert_eq!(
            token.reasons,
            vec!["補助動詞".to_string(), "過去".to_string()]
        );
        assert_eq!(token.source_pos, Some(LinderaPos::AuxVerb));
    }

    #[test]
    fn te_iku_becomes_aspectual_auxiliary() {
        let dict = Dictionary::from_entries(vec![
            entry(&["変わる"], &["かわる"], "v5r", &["to change"]),
            entry_with_common(&[], &["て"], "prt", &["connective te-form particle"], true),
            entry(&["畏懼"], &["いく"], "n", &["awe"]),
            entry(&["行く"], &["いく"], "v5k-s", &["to go"]),
        ]);
        let tokens = dict.analyze_line("変わっていく");
        let token = tokens
            .iter()
            .find(|token| token.surface == "いく")
            .unwrap_or_else(|| panic!("いく token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "いく");
        assert_eq!(token.reasons, vec!["補助動詞".to_string()]);
        assert_eq!(
            token.entries[0].senses[0].part_of_speech,
            vec!["v5k-s".to_string(), "vi".to_string()]
        );
    }

    #[test]
    fn dekiru_stem_prefers_ability_over_out_of_stock_homograph() {
        let dict = Dictionary::from_entries(vec![
            entry(&["できる"], &["できる"], "v5r", &["to be out of"]),
            entry(&["無い"], &["ない"], "aux-adj", &["not"]),
        ]);
        let tokens = dict.analyze_line("できない");
        let token = tokens
            .iter()
            .find(|token| token.surface == "でき")
            .unwrap_or_else(|| panic!("でき token in {tokens:?}"));
        assert_eq!(token.dictionary_form, "できる");
        assert_eq!(token.reasons, vec!["未然形".to_string()]);
        assert_eq!(
            token.entries[0].senses[0].part_of_speech,
            vec!["v1".to_string(), "vi".to_string()]
        );
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["to be able to; can  (v1, vi)".to_string()]
        );
    }

    #[test]
    fn suru_te_iru_becomes_progressive_auxiliary() {
        let dict = Dictionary::from_entries(vec![
            entry(&["入力"], &["にゅうりょく"], "vs", &["input"]),
            entry(&["する"], &["する"], "vs-i", &["to do"]),
            entry_with_common(&[], &["して"], "prt", &["by means of"], true),
            entry(&["いる"], &["いる"], "v1", &["to be"]),
        ]);
        let tokens = dict.analyze_line("入力している");
        let shite = tokens
            .iter()
            .find(|token| token.surface == "して")
            .unwrap_or_else(|| panic!("して token in {tokens:?}"));
        assert_eq!(shite.dictionary_form, "する");

        let iru = tokens
            .iter()
            .find(|token| token.surface == "いる")
            .unwrap_or_else(|| panic!("いる token in {tokens:?}"));
        assert_eq!(iru.dictionary_form, "いる");
        assert_eq!(iru.source_pos, Some(LinderaPos::AuxVerb));
    }

    #[test]
    fn policy_homographs_use_learner_primary_metadata() {
        let normalized = normalize_policy_homographs(vec![
            token("ください", "くださる", "v5aru"),
            token("済", "済", "n"),
            token("ねぇ", "ねぇ", "prt"),
            token("もの", "物", "n"),
            token("いい", "いい", "adj-t"),
            token("いた", "いた", "n"),
            token("なし", "ない", "adj-i"),
            token("なき", "無き", "adj-i"),
        ]);

        assert_eq!(normalized[0].dictionary_form, "ください");
        assert_eq!(
            normalized[0].entries[0].senses[0].part_of_speech,
            vec!["aux-v".to_string()]
        );
        assert_eq!(normalized[1].dictionary_form, "済み");
        assert_eq!(
            normalized[1].entries[0].senses[0].part_of_speech,
            vec!["n-suf".to_string()]
        );
        assert_eq!(normalized[2].dictionary_form, "ね");
        assert_eq!(
            normalized[2].entries[0].senses[0].part_of_speech,
            vec!["prt".to_string()]
        );
        assert_eq!(normalized[3].dictionary_form, "もの");
        assert_eq!(
            normalized[3].entries[0].senses[0].part_of_speech,
            vec!["n".to_string()]
        );
        assert_eq!(
            normalized[3].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["thing; object; matter  (n)".to_string()]
        );
        assert_eq!(normalized[4].dictionary_form, "いい");
        assert_eq!(
            normalized[4].entries[0].senses[0].part_of_speech,
            vec!["adj-i".to_string()]
        );
        assert_eq!(normalized[5].dictionary_form, "いる");
        assert_eq!(
            normalized[5].entries[0].senses[0].part_of_speech,
            vec!["v1".to_string(), "vi".to_string()]
        );
        assert_eq!(normalized[6].dictionary_form, "なし");
        assert_eq!(
            normalized[6].entries[0].senses[0].part_of_speech,
            vec!["n".to_string(), "n-suf".to_string()]
        );
        assert_eq!(normalized[7].dictionary_form, "ない");
        assert_eq!(
            normalized[7].entries[0].senses[0].part_of_speech,
            vec!["adj-i".to_string()]
        );
    }

    #[test]
    fn policy_particles_use_single_learner_primary_glosses() {
        let particles = ["の", "に", "を", "は", "が", "で", "へ", "から", "も", "と"];
        let normalized = normalize_policy_homographs(
            particles
                .iter()
                .map(|surface| token(surface, surface, "prt"))
                .collect(),
        );

        for token in normalized {
            assert_eq!(token.entries[0].senses[0].part_of_speech, vec!["prt"]);
            assert_eq!(
                token.entries[0]
                    .popup_override
                    .as_ref()
                    .expect("popup")
                    .glosses
                    .len(),
                1
            );
            assert_eq!(
                token.entries[0]
                    .popup_override
                    .as_ref()
                    .expect("popup")
                    .glosses[0],
                canonical_particle_gloss(&token.surface)
                    .expect("canonical particle")
                    .to_string()
            );
        }
    }

    #[test]
    fn policy_te_particle_uses_connective_sense() {
        let normalized = normalize_policy_homographs(vec![token("て", "て", "prt")]);
        let token = &normalized[0];

        assert_eq!(token.dictionary_form, "て");
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["and; then; -ing (connective)  (prt)".to_string()]
        );
    }

    #[test]
    fn policy_grammar_overrides_suppress_misleading_homographs() {
        let normalized = normalize_policy_homographs(vec![
            token("こと", "こと", "n"),
            token("する", "する", "vs-i"),
            token("です", "です", "cop"),
            token("この", "この", "adj-pn"),
            token("ただ", "ただ", "adv"),
            token("ない", "ない", "adj-i"),
        ]);

        assert_eq!(normalized[0].dictionary_form, "こと");
        assert_eq!(
            normalized[0].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["thing; matter; act; fact; nominalizer  (n)".to_string()]
        );
        assert!(
            !normalized[0].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses
                .iter()
                .any(|gloss| gloss.contains("zither"))
        );
        assert_eq!(
            normalized[1].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["to do; to carry out; to perform  (vs-i)".to_string()]
        );
        assert_eq!(
            normalized[2].entries[0].senses[0].part_of_speech,
            vec!["cop".to_string(), "aux-v".to_string()]
        );
        assert_eq!(
            normalized[3].entries[0].senses[0].part_of_speech,
            vec!["adj-pn"]
        );
        assert_eq!(
            normalized[4].entries[0].senses[0].part_of_speech,
            vec!["adv"]
        );
        assert_eq!(
            normalized[5].entries[0].senses[0].part_of_speech,
            vec!["adj-i"]
        );
    }

    #[test]
    fn domain_game_terms_use_concise_learner_primary_glosses() {
        let normalized = merge_domain_terms(vec![
            token("攻撃", "攻撃", "n"),
            token("時間", "時間", "n"),
            token("敵", "敵", "n"),
            token("数", "数", "adv"),
            token("必要", "必要", "adj-na"),
            token("特定", "特定", "adj-no"),
            token("終奏", "終奏", "n"),
            token("重撃", "重撃", "n"),
            token("ラウンド", "ラウンド", "adj-f"),
            token("エンド", "エンド", "conj"),
            token("リンク状態", "リンク状態", "unc"),
        ]);

        let attack = &normalized[0];
        assert_eq!(
            attack.entries[0].popup_override.as_ref().unwrap().glosses,
            vec!["attack; assault; strike  (n, vs, vt)".to_string()]
        );
        assert!(
            !attack.entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses
                .iter()
                .any(|gloss| gloss.contains("cyber") || gloss.contains("criticism"))
        );
        assert_eq!(
            normalized[1].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["time; hour  (n)".to_string()]
        );
        assert_eq!(
            normalized[2].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["enemy; opponent; adversary  (n)".to_string()]
        );
        assert_eq!(
            normalized[3].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .ruby,
            vec![RubySegment {
                text: "数".to_string(),
                furigana: Some("かず".to_string())
            }]
        );
        assert_eq!(
            normalized[4].entries[0].senses[0].part_of_speech,
            vec!["n".to_string(), "adj-na".to_string()]
        );
        assert_eq!(
            normalized[5].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["specifying; identifying; pinpointing  (n, vs, vt, adj-no)".to_string()]
        );
        assert_eq!(normalized[6].dictionary_form, "終奏");
        assert!(normalized[6].is_known());
        assert_eq!(
            normalized[7].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["heavy attack; heavy strike  (n)".to_string()]
        );
        assert_eq!(
            normalized[7].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .ruby,
            vec![RubySegment {
                text: "重撃".to_string(),
                furigana: Some("じゅうげき".to_string())
            }]
        );
        assert_eq!(
            normalized[8].entries[0].senses[0].part_of_speech,
            vec!["n".to_string()]
        );
        assert_eq!(
            normalized[8].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["round  (n)".to_string()]
        );
        assert_eq!(
            normalized[9].entries[0].senses[0].part_of_speech,
            vec!["n".to_string()]
        );
        assert_eq!(
            normalized[9].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["end  (n)".to_string()]
        );
        assert_eq!(
            normalized[10].entries[0].senses[0].part_of_speech,
            vec!["n".to_string()]
        );
        assert_eq!(
            normalized[10].entries[0]
                .popup_override
                .as_ref()
                .unwrap()
                .glosses,
            vec!["link state  (n)".to_string()]
        );
    }

    #[test]
    fn lexicalized_deverbal_nouns_use_noun_headwords() {
        let normalized = merge_domain_terms(vec![
            token("誓い", "誓う", "v5u"),
            token("まどろみ", "まどろむ", "v5m"),
            token("轟き", "轟く", "v5k"),
            token("いざない", "いざなう", "v5u"),
            token("導き", "導く", "v5k"),
            token("行い", "行う", "v5u"),
        ]);

        assert_eq!(normalized[0].dictionary_form, "誓い");
        assert_eq!(normalized[0].entries[0].senses[0].part_of_speech, vec!["n"]);
        assert_eq!(normalized[1].dictionary_form, "まどろみ");
        assert_eq!(normalized[2].dictionary_form, "轟き");
        assert_eq!(normalized[3].dictionary_form, "誘い");
        assert_eq!(normalized[4].dictionary_form, "導き");
        assert_eq!(normalized[5].dictionary_form, "行う");
    }

    #[test]
    fn ascii_abbreviation_unknowns_are_other_without_dictionary_entry() {
        let token = unknown_surface_token("EXP".to_string());

        assert!(!token.is_known());
        assert_eq!(token.source_pos, Some(LinderaPos::Other));
    }

    #[test]
    fn status_chuu_suffix_prefers_during_reading_after_noun() {
        let normalized =
            normalize_policy_homographs(vec![token("状態", "状態", "n"), token("中", "中", "n")]);
        let token = &normalized[1];

        assert_eq!(token.dictionary_form, "中");
        assert_eq!(
            token.entries[0].senses[0].part_of_speech,
            vec!["n".to_string(), "suf".to_string()]
        );
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .ruby,
            vec![RubySegment {
                text: "中".to_string(),
                furigana: Some("ちゅう".to_string())
            }]
        );
    }

    #[test]
    fn standalone_naka_context_stays_inside_reading() {
        let normalized =
            normalize_policy_homographs(vec![token("の", "の", "prt"), token("中", "中", "n")]);
        let token = &normalized[1];

        assert_eq!(token.dictionary_form, "中");
        assert_eq!(token.entries[0].senses[0].part_of_speech, vec!["n"]);
        assert!(token.entries[0].popup_override.is_none());
    }

    #[test]
    fn day_after_numeral_prefers_counter_reading() {
        let normalized = normalize_policy_homographs(vec![
            unknown_surface_token("9".to_string()),
            token("日", "日", "n"),
            token("時間", "時間", "n"),
        ]);
        let token = &normalized[1];

        assert_eq!(
            token.entries[0].senses[0].part_of_speech,
            vec!["n".to_string(), "ctr".to_string()]
        );
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .ruby,
            vec![RubySegment {
                text: "日".to_string(),
                furigana: Some("にち".to_string())
            }]
        );
    }

    #[test]
    fn person_after_numeral_prefers_counter_reading() {
        let normalized = normalize_policy_homographs(vec![
            unknown_surface_token("3".to_string()),
            token("人", "人", "n"),
            token("敵", "敵", "n"),
        ]);
        let counter = &normalized[1];

        assert_eq!(counter.entries[0].senses[0].part_of_speech, vec!["ctr"]);
        assert_eq!(
            counter.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .ruby,
            vec![RubySegment {
                text: "人".to_string(),
                furigana: Some("にん".to_string())
            }]
        );

        let standalone = normalize_policy_homographs(vec![token("人", "人", "n")]);
        assert_eq!(standalone[0].entries[0].senses[0].part_of_speech, vec!["n"]);
    }

    #[test]
    fn mei_after_numeral_prefers_people_counter_reading() {
        let normalized = normalize_policy_homographs(vec![
            unknown_surface_token("10".to_string()),
            token("名", "名", "n"),
        ]);
        let counter = &normalized[1];

        assert_eq!(counter.entries[0].senses[0].part_of_speech, vec!["ctr"]);
        assert_eq!(
            counter.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .ruby,
            vec![RubySegment {
                text: "名".to_string(),
                furigana: Some("めい".to_string())
            }]
        );

        let standalone = normalize_policy_homographs(vec![token("名", "名", "n")]);
        assert_eq!(standalone[0].entries[0].senses[0].part_of_speech, vec!["n"]);
    }

    #[test]
    fn nonlexical_wave_dash_keeps_symbol_without_dictionary_gloss() {
        let normalized = normalize_policy_homographs(vec![token("〜", "〜", "unc")]);
        let token = &normalized[0];

        assert!(token.is_known());
        assert_eq!(token.entries[0].senses[0].part_of_speech, vec!["unc"]);
        let popup = token.entries[0].popup_override.as_ref().expect("popup");
        assert!(popup.ruby.is_empty());
        assert!(popup.glosses.is_empty());
    }

    #[test]
    fn honorific_suffixes_suppress_common_noun_homographs_after_names() {
        let normalized = normalize_policy_homographs(vec![
            unknown_surface_token("ダーニャ".to_string()),
            token("さん", "さん", "n"),
            token("団子", "団子", "n"),
            token("ちゃん", "ちゃん", "n"),
        ]);

        assert_eq!(
            normalized[1].entries[0].senses[0].part_of_speech,
            vec!["suf"]
        );
        assert_eq!(
            normalized[1].entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["Mr.; Mrs.; Miss; Ms.; -san  (suf)".to_string()]
        );
        assert_eq!(
            normalized[3].entries[0].senses[0].part_of_speech,
            vec!["suf"]
        );

        let standalone = normalize_policy_homographs(vec![token("さん", "さん", "n")]);
        assert_eq!(standalone[0].entries[0].senses[0].part_of_speech, vec!["n"]);
    }

    #[test]
    fn nda_prefers_explanatory_expression_over_interjection() {
        let normalized = normalize_policy_homographs(vec![token("んだ", "んだ", "int")]);
        let token = &normalized[0];

        assert_eq!(token.entries[0].senses[0].part_of_speech, vec!["exp"]);
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["the fact is; it is that ...  (exp)".to_string()]
        );
    }

    #[test]
    fn clipped_kirikae_uses_learner_dictionary_form() {
        let normalized = normalize_policy_homographs(vec![token("切替", "切替", "n")]);
        let token = &normalized[0];

        assert_eq!(token.dictionary_form, "切り替え");
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["switching; exchange; changeover  (n)".to_string()]
        );
    }

    #[test]
    fn kuru_kana_prefers_come_over_rare_homograph() {
        let normalized = normalize_policy_homographs(vec![token("くる", "くる", "v5r")]);
        let token = &normalized[0];

        assert_eq!(token.dictionary_form, "来る");
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["to come; to arrive; to come along  (vk, vi)".to_string()]
        );
    }

    #[test]
    fn kita_after_te_de_prefers_come_helper_over_north() {
        let normalized = normalize_policy_homographs(vec![
            token("歩んで", "歩む", "v5m"),
            token("きた", "きた", "n"),
        ]);
        let come = &normalized[1];

        assert_eq!(come.dictionary_form, "来る");
        assert_eq!(come.reasons, vec!["過去".to_string()]);
        assert_eq!(
            come.note_override.as_deref(),
            Some("Past form of 来る in a て/でくる chain.")
        );

        let standalone = normalize_policy_homographs(vec![token("きた", "きた", "n")]);
        assert_eq!(standalone[0].dictionary_form, "きた");
    }

    #[test]
    fn naku_prefers_negative_auxiliary_over_dead_homograph() {
        let normalized = normalize_policy_homographs(vec![token("なく", "ない", "adj-i")]);
        let token = &normalized[0];

        assert_eq!(token.dictionary_form, "ない");
        assert_eq!(
            token.entries[0].senses[0].part_of_speech,
            vec!["aux-adj".to_string()]
        );
        assert_eq!(
            token.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["not; non-; un-  (aux-adj)".to_string()]
        );
    }

    #[test]
    fn nara_after_nominal_prefers_conditional_particle() {
        let normalized = normalize_policy_homographs(vec![
            token("Jess", "Jess", "n"),
            token("ちゃん", "ちゃん", "n"),
            token("なら", "だ", "aux-v"),
        ]);
        let nara = &normalized[2];

        assert_eq!(nara.dictionary_form, "なら");
        assert_eq!(
            nara.entries[0].senses[0].part_of_speech,
            vec!["prt".to_string()]
        );
        assert_eq!(
            nara.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["if; in case of; as for  (prt)".to_string()]
        );

        let standalone = normalize_policy_homographs(vec![token("なら", "だ", "aux-v")]);
        assert_eq!(standalone[0].dictionary_form, "だ");
    }

    #[test]
    fn bound_prefixes_prefer_prefix_sense_only_before_nominals() {
        let normalized = normalize_policy_homographs(vec![
            token("非", "非", "n"),
            token("深層", "深層", "n"),
            token("古", "古", "n"),
            token("鐘", "鐘", "n"),
        ]);

        assert_eq!(
            normalized[0].entries[0].senses[0].part_of_speech,
            vec!["pref".to_string()]
        );
        assert_eq!(
            normalized[0].entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["non-; un-; anti-  (pref)".to_string()]
        );
        assert_eq!(
            normalized[2].entries[0].senses[0].part_of_speech,
            vec!["pref".to_string()]
        );
        assert_eq!(
            normalized[2].entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["old; ancient  (pref)".to_string()]
        );

        let standalone = normalize_policy_homographs(vec![token("古", "古", "n")]);
        assert_eq!(standalone[0].entries[0].senses[0].part_of_speech, vec!["n"]);
    }

    #[test]
    fn contracted_teru_after_te_form_prefers_te_iru_auxiliary() {
        let dict = Dictionary::from_entries(Vec::new());
        let normalized = dict.normalize_te_iru_auxiliary_tokens(vec![
            token("て", "て", "prt"),
            token("てる", "てる", "v5r"),
        ]);
        let teru = &normalized[1];

        assert_eq!(teru.dictionary_form, "ている");
        assert_eq!(teru.reasons, vec!["口語短縮".to_string()]);
        assert_eq!(
            teru.entries[0].senses[0].part_of_speech,
            vec!["aux-v".to_string()]
        );
        assert_eq!(teru.note_override.as_deref(), Some("てる · 口語短縮"));

        let mut te_contracted_stem = token("持っ", "持つ", "v5t");
        te_contracted_stem.reasons = vec!["連用タ接続".to_string()];
        let contracted = dict.normalize_te_iru_auxiliary_tokens(vec![
            te_contracted_stem,
            token("てる", "てる", "v5r"),
        ]);
        assert_eq!(contracted[1].dictionary_form, "ている");

        let standalone = dict.normalize_te_iru_auxiliary_tokens(vec![token("てる", "てる", "v5r")]);
        assert_eq!(standalone[0].dictionary_form, "てる");
    }

    #[test]
    fn sareru_after_suru_nominal_prefers_passive_auxiliary() {
        let dict = Dictionary::from_entries(Vec::new());
        let mut reset = token("リセット", "リセット", "n");
        reset.entries[0].senses[0].part_of_speech = vec!["n".to_string(), "vs".to_string()];
        let normalized =
            dict.normalize_suru_passive_form_tokens(vec![reset, token("される", "される", "v1")]);
        let sareru = &normalized[1];

        assert_eq!(sareru.dictionary_form, "される");
        assert_eq!(
            sareru.entries[0].senses[0].part_of_speech,
            vec!["aux-v".to_string(), "v1".to_string()]
        );
        assert_eq!(
            sareru.entries[0]
                .popup_override
                .as_ref()
                .expect("popup")
                .glosses,
            vec!["to be ...-ed; passive of する  (aux-v, v1)".to_string()]
        );

        let standalone =
            dict.normalize_suru_passive_form_tokens(vec![token("される", "される", "v1")]);
        assert_eq!(
            standalone[0].entries[0].senses[0].part_of_speech,
            vec!["v1"]
        );
    }

    #[test]
    fn single_kana_auxiliary_fragment_does_not_resolve_to_content_homograph() {
        let dict = Dictionary::from_entries(vec![entry(&["り"], &["り"], "n", &["li"])]);
        for major_pos in ["助動詞", "名詞"] {
            let morpheme = Morpheme {
                surface: "る".to_string(),
                base_form: "り".to_string(),
                reading: Some("ル".to_string()),
                pos: vec![major_pos.to_string(), "*".to_string()],
                conjugation_form: None,
            };
            let (_, token) = dict
                .node(&[morpheme], 0, 1)
                .expect("single morpheme resolves as unknown");

            assert_eq!(token.surface, "る");
            assert_eq!(token.dictionary_form, "る");
            assert!(!token.is_known());
        }
    }

    #[test]
    fn merges_honorific_prefix_with_known_base_word() {
        let dict = Dictionary::from_entries(vec![
            entry(&["御"], &["ご"], "pref", &["honorific prefix"]),
            entry(
                &["利用"],
                &["りよう"],
                "n",
                &["use; utilization; utilisation; application"],
            ),
        ]);
        let token = dict
            .analyze_line("ご利用")
            .into_iter()
            .next()
            .expect("token");
        assert_eq!(token.surface, "ご利用");
        assert_eq!(token.dictionary_form, "利用");
        assert_eq!(
            token.note_override.as_deref(),
            Some("Honorific ご prefix on 利用.")
        );
        let popup = token.entries[0].popup_override.as_ref().expect("popup");
        assert_eq!(
            popup.ruby,
            vec![
                RubySegment {
                    text: "ご".to_string(),
                    furigana: None,
                },
                RubySegment {
                    text: "利用".to_string(),
                    furigana: Some("りよう".to_string()),
                },
            ]
        );
        assert_eq!(
            popup.glosses,
            vec!["use; utilization; utilisation; application  (n)".to_string()]
        );
    }

    #[test]
    fn keeps_surface_spelling_over_homograph_headword() {
        // 本 is registered under both a "book" entry and an "origin" (元) entry.
        // The resolved form must report 本, not the other entry's headword 元.
        let dict = Dictionary::from_entries(vec![
            entry(&["元", "本"], &["もと"], "n", &["origin"]),
            entry(&["本"], &["ほん"], "n", &["book"]),
        ]);
        let token = dict
            .analyze_line("本")
            .into_iter()
            .find(Token::is_known)
            .expect("known token");
        assert_eq!(token.dictionary_form, "本");
        assert!(token.reasons.is_empty());
        assert_eq!(token.entries.len(), 2);
    }

    #[test]
    fn particle_prefers_particle_sense_over_noun_homograph() {
        // は has a frequent noun homograph (羽 "feather") but Lindera tags it
        // 助詞; the resolved token must surface the particle sense first so the
        // popup/category show "topic marker", not "feather".
        let dict = Dictionary::from_entries(vec![
            entry(&["今日"], &["きょう"], "n", &["today"]),
            entry(&["羽", "羽根"], &["はね"], "n", &["feather"]),
            entry_with_common(&[], &["は"], "prt", &["indicates sentence topic"], true),
        ]);
        let token = dict
            .analyze_line("今日は")
            .into_iter()
            .find(|token| token.surface == "は")
            .expect("は token");
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["prt".to_string()]);
    }

    #[test]
    fn auxiliary_prefers_aux_sense_over_noun_homograph() {
        // The volitional う (助動詞) shares a surface with the noun 鵜 (cormorant).
        let dict = Dictionary::from_entries(vec![
            entry(&["帰る"], &["かえる"], "v5r", &["to return"]),
            entry(&["鵜"], &["う"], "n", &["cormorant"]),
            entry_with_common(
                &[],
                &["う"],
                "aux-v",
                &["indicates speaker's volition"],
                true,
            ),
        ]);
        let token = dict
            .analyze_line("帰ろう")
            .into_iter()
            .find(|token| token.surface == "う")
            .expect("う token");
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["aux-v".to_string()]);
    }

    #[test]
    fn polite_past_auxiliary_chain_beats_surface_noun_homograph() {
        let dict = Dictionary::from_entries(vec![
            entry(&["飲む"], &["のむ"], "v5m", &["to drink"]),
            entry(&["真下", "ました"], &["ました"], "n", &["directly below"]),
        ]);
        let token = dict
            .analyze_line("飲みました")
            .into_iter()
            .find(|token| token.surface == "ました")
            .expect("polite-past auxiliary token");
        assert_eq!(token.dictionary_form, "ます");
        assert!(!token.is_known());
        assert_eq!(token.source_pos, Some(LinderaPos::AuxVerb));
        assert_eq!(token.reasons, vec!["丁寧".to_string(), "過去".to_string()]);
        assert_eq!(
            token.note_override.as_deref(),
            Some("Polite past auxiliary.")
        );
    }

    #[test]
    fn single_morpheme_potential_contraction_resolves_to_godan_lemma() {
        let dict = Dictionary::from_entries(vec![entry(&["読む"], &["よむ"], "v5m", &["to read"])]);
        let token = dict
            .analyze_line("読める")
            .into_iter()
            .find(|token| token.surface == "読める")
            .expect("deinflected potential token");

        assert_eq!(token.dictionary_form, "読む");
        assert!(token.is_known());
        assert_eq!(token.reasons, vec!["可能".to_string()]);
    }

    #[test]
    fn exact_headword_beats_deinflection_candidate() {
        let dict = Dictionary::from_entries(vec![
            entry(&["焼ける"], &["やける"], "v1", &["to burn"]),
            entry(&["焼く"], &["やく"], "v5k", &["to grill"]),
        ]);
        let token = dict
            .analyze_line("焼ける")
            .into_iter()
            .find(|token| token.surface == "焼ける")
            .expect("exact lexical token");

        assert_eq!(token.dictionary_form, "焼ける");
        assert!(token.is_known());
        assert!(token.reasons.is_empty());
    }

    #[test]
    fn suru_stem_deinflection_beats_godan_s_lemma() {
        let dict = Dictionary::from_entries(vec![
            entry(&["達す"], &["たっす"], "v5s", &["to reach"]),
            entry(&["達する"], &["たっする"], "vs-s", &["to reach"]),
        ]);
        let morpheme = Morpheme {
            surface: "達し".to_string(),
            base_form: "達す".to_string(),
            reading: Some("タッシ".to_string()),
            pos: vec!["動詞".to_string(), "自立".to_string()],
            conjugation_form: Some("連用形".to_string()),
        };

        let (_, token) = dict
            .node(&[morpheme], 0, 1)
            .expect("single morpheme resolves");

        assert_eq!(token.dictionary_form, "達する");
        assert_eq!(token.reasons, vec!["連用形".to_string()]);
    }

    #[test]
    fn recursive_deinflection_engine_handles_complex_auxiliary_chain() {
        let forms = deinflection::deinflect("食べさせられたくなかった")
            .into_iter()
            .map(|candidate| candidate.form)
            .collect::<Vec<_>>();
        assert!(forms.contains(&"食べる".to_string()), "{forms:?}");
    }

    #[test]
    fn contracted_deinflection_engine_handles_te_auxiliary_shortening() {
        let dict =
            Dictionary::from_entries(vec![entry(&["走る"], &["はしる"], "v5r", &["to run"])]);
        let candidate = deinflection::deinflect("走ってた")
            .into_iter()
            .find(|candidate| {
                candidate.form == "走る"
                    && dict.entries_for(&candidate.form).is_some_and(|entries| {
                        entries.into_iter().any(|entry| {
                            deinflection::entry_matches_type(entry, candidate.word_type)
                        })
                    })
            })
            .expect("contracted godan candidate");

        assert_eq!(
            candidate.reasons,
            vec!["継続".to_string(), "過去".to_string()]
        );
    }

    #[test]
    fn particle_is_not_fused_into_noun_homograph() {
        // 待っていて segments as 待っ / て / い / て; the trailing い + て must not
        // fuse into the noun 射手 (いて, "archer"), a grammatical false-merge.
        let dict = Dictionary::from_entries(vec![
            entry(&["待つ"], &["まつ"], "v5t", &["to wait"]),
            entry(&["居る"], &["いる"], "v1", &["to be (animate)"]),
            entry(&["射手"], &["いて"], "n", &["archer"]),
        ]);
        let tokens = dict.analyze_line("待っていて");
        assert!(
            tokens.iter().all(|token| token.dictionary_form != "射手"),
            "いて wrongly fused into 射手: {tokens:?}"
        );
    }

    #[test]
    fn contentful_compound_with_particle_still_fuses() {
        // The guard must not block legitimate particle-bearing compounds: 一緒に
        // (一緒 + に, adv) and 手に入れる (exp,v1) should still fuse to one token.
        let dict = Dictionary::from_entries(vec![
            entry(&["一緒"], &["いっしょ"], "n", &["together"]),
            entry(&["一緒に"], &["いっしょに"], "adv", &["together (with)"]),
            entry(&["手に入れる"], &["てにいれる"], "exp", &["to obtain"]),
        ]);
        assert!(
            known_forms(&dict, "一緒に").contains(&"一緒に".to_string()),
            "一緒に should fuse"
        );
        assert!(
            known_forms(&dict, "手に入れる").contains(&"手に入れる".to_string()),
            "手に入れる should fuse"
        );
    }

    #[test]
    fn unknown_tokens_have_no_entries() {
        let dict = sample_dict();
        let tokens = dict.analyze_line("食べるXYZ");
        assert!(
            tokens
                .iter()
                .any(|token| token.is_known() && token.dictionary_form == "食べる")
        );
        assert!(tokens.iter().any(|token| !token.is_known()));
    }
}
