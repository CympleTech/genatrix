//! What a manifest allows, one line per grant, in the reader's language
//! (design 11, "装之前": the manifest said in words). Written out rather
//! than assembled from fragments, because a sentence translated a piece
//! at a time reads like one.

use genatrix_host::manifest::{
    BlobAccess, Effect, Manifest, Purpose, TargetRule, Targets, Trigger,
};

/// Which words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// English.
    En,
    /// Chinese.
    Zh,
}

impl Lang {
    /// From an `Accept-Language` header: Chinese if the reader prefers it.
    #[must_use]
    pub fn from_header(value: Option<&str>) -> Self {
        match value {
            Some(v) if v.trim_start().to_ascii_lowercase().starts_with("zh") => Self::Zh,
            _ => Self::En,
        }
    }
}

fn level(l: genatrix_model::Level, lang: Lang) -> &'static str {
    use genatrix_model::Level::{Personal, Public, Secret};
    match (l, lang) {
        (Public, Lang::En) => "public",
        (Personal, Lang::En) => "personal",
        (Secret, Lang::En) => "secret",
        (Public, Lang::Zh) => "公开",
        (Personal, Lang::Zh) => "个人",
        (Secret, Lang::Zh) => "机密",
    }
}

fn connector(c: &str, lang: Lang) -> String {
    match (c, lang) {
        ("imap", Lang::En) => "your mailboxes".into(),
        ("imap", Lang::Zh) => "邮箱".into(),
        ("telegram", _) => "Telegram".into(),
        (other, _) => other.to_owned(),
    }
}

fn kind(k: &str, lang: Lang) -> String {
    match (k, lang) {
        ("mail", Lang::En) => "mails".into(),
        ("message", Lang::En) => "messages".into(),
        ("event", Lang::En) => "events".into(),
        ("mail", Lang::Zh) => "邮件".into(),
        ("message", Lang::Zh) => "消息".into(),
        ("event", Lang::Zh) => "日程".into(),
        (other, _) => other.to_owned(),
    }
}

fn purpose(p: Purpose, lang: Lang) -> &'static str {
    match (p, lang) {
        (Purpose::Classify, Lang::En) => "classify",
        (Purpose::Extract, Lang::En) => "extract",
        (Purpose::Summarize, Lang::En) => "summarize",
        (Purpose::Draft, Lang::En) => "draft",
        (Purpose::Translate, Lang::En) => "translate",
        (Purpose::Classify, Lang::Zh) => "分类",
        (Purpose::Extract, Lang::Zh) => "提取",
        (Purpose::Summarize, Lang::Zh) => "摘要",
        (Purpose::Draft, Lang::Zh) => "起草",
        (Purpose::Translate, Lang::Zh) => "翻译",
    }
}

fn every(e: &str, at: &str, lang: Lang) -> String {
    let day = |d: &str| match (d, lang) {
        ("mon", Lang::Zh) => "周一",
        ("tue", Lang::Zh) => "周二",
        ("wed", Lang::Zh) => "周三",
        ("thu", Lang::Zh) => "周四",
        ("fri", Lang::Zh) => "周五",
        ("sat", Lang::Zh) => "周六",
        ("sun", Lang::Zh) => "周日",
        (d, _) => match d {
            "mon" => "Monday",
            "tue" => "Tuesday",
            "wed" => "Wednesday",
            "thu" => "Thursday",
            "fri" => "Friday",
            "sat" => "Saturday",
            _ => "Sunday",
        },
    };
    match (e.split_once(':'), lang) {
        (None, Lang::En) => format!("every day at {at}"),
        (None, Lang::Zh) => format!("每天 {at}"),
        (Some(("weekly", d)), Lang::En) => format!("every {} at {at}", day(d)),
        (Some(("weekly", d)), Lang::Zh) => format!("每{} {at}", day(d)),
        (Some((_, d)), Lang::En) => format!("on day {d} of each month at {at}"),
        (Some((_, d)), Lang::Zh) => format!("每月 {d} 日 {at}"),
    }
}

fn join(parts: &[String], lang: Lang) -> String {
    parts.join(if lang == Lang::Zh { "、" } else { ", " })
}

/// One line per grant: `reads`, `takes`, `proposes`, `model`, `runs`,
/// `cannot`.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one sentence per grant, both languages side by side"
)]
pub fn describe(m: &Manifest, lang: Lang) -> Vec<(&'static str, String)> {
    let zh = lang == Lang::Zh;
    let r = &m.reads;
    let reads = if r.connectors.is_empty() {
        if zh {
            "什么都不读".into()
        } else {
            "nothing".into()
        }
    } else {
        let sources = join(
            &r.connectors
                .iter()
                .map(|c| connector(c, lang))
                .collect::<Vec<_>>(),
            lang,
        );
        let kinds = if r.kinds.is_empty() {
            if zh {
                "全部内容".to_owned()
            } else {
                "everything".to_owned()
            }
        } else {
            join(
                &r.kinds.iter().map(|k| kind(k, lang)).collect::<Vec<_>>(),
                lang,
            )
        };
        let attachment = match (r.with_attachment, r.mime.is_empty(), zh) {
            (false, _, _) => String::new(),
            (true, true, false) => " with an attachment".into(),
            (true, true, true) => "带附件的".into(),
            (true, false, false) => format!(" with a {} attachment", r.mime.join(" or ")),
            (true, false, true) => format!("带 {} 附件的", r.mime.join(" 或 ")),
        };
        let matching = match (r.matching.is_empty(), zh) {
            (true, _) => String::new(),
            (false, false) => format!(" mentioning {}", r.matching.join(" or ")),
            (false, true) => format!("提到 {} 的", r.matching.join(" 或 ")),
        };
        let window = match (r.days, zh) {
            (None, false) => "all history".to_owned(),
            (None, true) => "全部历史".to_owned(),
            (Some(d), false) => format!("the last {d} days"),
            (Some(d), true) => format!("最近 {d} 天"),
        };
        let top = level(r.max_level, lang);
        if zh {
            // "最近 7 天的邮件", but "最近 7 天带附件的邮件": one 的 either way.
            let of = if attachment.is_empty() && matching.is_empty() {
                "的"
            } else {
                ""
            };
            format!("{sources}里{window}{attachment}{matching}{of}{kinds}，最高到{top}级")
        } else {
            format!("{kinds} from {sources}{attachment}{matching}, from {window}, up to {top}")
        }
    };
    let takes = match (m.blobs, zh) {
        (BlobAccess::None, false) => "no attachments".to_owned(),
        (BlobAccess::None, true) => "不取附件".to_owned(),
        (BlobAccess::Text, false) => "the text of those attachments".to_owned(),
        (BlobAccess::Text, true) => "这些附件里的文字".to_owned(),
    };
    let proposes = if m.proposes.is_empty() {
        if zh {
            "什么都不提议".into()
        } else {
            "nothing".into()
        }
    } else {
        let each: Vec<String> = m
            .proposes
            .iter()
            .map(|p| {
                let reach = match (&p.effect, &p.targets, zh) {
                    (Effect::Own, _, false) => "in its own space".to_owned(),
                    (Effect::Own, _, true) => "写进它自己的空间".to_owned(),
                    (_, Some(Targets::Addresses(a)), false) => format!("only to {}", a.join(", ")),
                    (_, Some(Targets::Addresses(a)), true) => format!("只发给 {}", a.join("、")),
                    (_, Some(Targets::Rule(TargetRule::SameThread)), false) => {
                        "only as a reply in the same thread".into()
                    }
                    (_, Some(Targets::Rule(TargetRule::SameThread)), true) => {
                        "只能在原线程里回复".into()
                    }
                    (_, Some(Targets::Rule(TargetRule::KnownContacts)), false) => {
                        "only to people you have written to".into()
                    }
                    (_, Some(Targets::Rule(TargetRule::KnownContacts)), true) => {
                        "只发给你写过信的人".into()
                    }
                    (_, None, false) => "outside".into(),
                    (_, None, true) => "对外".into(),
                };
                if zh {
                    format!("{}（{reach}），需要你批准", p.label)
                } else {
                    format!("{} ({reach}), with your approval", p.label)
                }
            })
            .collect();
        each.join(if zh { "；" } else { "; " })
    };
    let model = if m.model.is_empty() {
        if zh {
            "不用模型".into()
        } else {
            "no model".into()
        }
    } else {
        let uses = join(
            &m.model
                .iter()
                .map(|p| purpose(*p, lang).to_owned())
                .collect::<Vec<_>>(),
            lang,
        );
        if zh {
            format!("只做{uses}；只在这台电脑上")
        } else {
            format!("{uses}; on this machine only")
        }
    };
    let runs: Vec<String> = m
        .triggers
        .iter()
        .map(|t| match (t, zh) {
            (Trigger::Items, false) => "when new items in scope arrive".to_owned(),
            (Trigger::Items, true) => "范围内有新数据到达时".to_owned(),
            (Trigger::Message, false) => "when you write to it".to_owned(),
            (Trigger::Message, true) => "你给它发消息时".to_owned(),
            (Trigger::Schedule { every: e, at, .. }, _) => every(e, at, lang),
        })
        .collect();
    let cannot = if zh {
        "联网、读其他数据、看其他 agent 的数据".to_owned()
    } else {
        "reach the network, read anything else, or see other agents' data".to_owned()
    };
    vec![
        ("reads", reads),
        ("takes", takes),
        ("proposes", proposes),
        ("model", model),
        ("runs", runs.join(if zh { "；" } else { "; " })),
        ("cannot", cannot),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_say_the_same_grants() {
        let m = genatrix_host::Manifest::parse(
            r#"
            name = "NZ tax"
            purpose = "p"
            author = "a"
            blobs = "text"
            model = ["extract"]
            [reads]
            connectors = ["imap"]
            kinds = ["mail"]
            with_attachment = true
            mime = ["application/pdf"]
            days = 365
            max_level = "secret"
            [[proposes]]
            kind = "record"
            label = "Record an entry"
            [[triggers]]
            on = "schedule"
            name = "gst"
            every = "monthly:1"
            at = "09:00"
            "#,
        )
        .unwrap();
        let en = describe(&m, Lang::En);
        let zh = describe(&m, Lang::Zh);
        assert_eq!(en.len(), zh.len());
        assert_eq!(
            en[0].1,
            "mails from your mailboxes with a application/pdf attachment, from the last 365 days, up to secret"
        );
        assert_eq!(
            zh[0].1,
            "邮箱里最近 365 天带 application/pdf 附件的邮件，最高到机密级"
        );
        assert_eq!(zh[4].1, "每月 1 日 09:00");
        assert_eq!(Lang::from_header(Some("zh-CN,zh;q=0.9")), Lang::Zh);
        assert_eq!(Lang::from_header(Some("en-NZ")), Lang::En);
    }
}
