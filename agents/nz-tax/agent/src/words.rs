//! What the agent says, in the language it was spoken to in.

use taxcore::Money;

use crate::books::{Gst101, Ir3};

/// Whether to answer in Chinese: the user wrote Chinese.
#[must_use]
pub fn chinese(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// An amount as people write it: `NZD 1,234.50`, `-NZD 12.00`.
#[must_use]
pub fn money(m: Money) -> String {
    let sign = if m.cents < 0 { "-" } else { "" };
    let abs = m.cents.unsigned_abs();
    let dollars = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in dollars.chars().enumerate() {
        if i > 0 && (dollars.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}{} {grouped}.{:02}", m.currency, abs % 100)
}

/// A GST return in words.
#[must_use]
pub fn gst101(r: &Gst101, zh: bool) -> String {
    let b = |n: u8| money(r.boxes[&n]);
    let to_pay = r.to_pay();
    let mut out = if zh {
        format!(
            "GST 申报期 {} 至 {}（{} 笔分录），{} 前申报。\n\
             第 5 栏 销售与收入（含税）：{}\n第 8 栏 销售中的 GST：{}\n\
             第 11 栏 采购与费用（含税）：{}\n第 12 栏 可抵扣的 GST：{}\n\
             第 15 栏 {}：{}",
            r.period.start,
            r.period.end,
            r.entries,
            r.due,
            b(5),
            b(8),
            b(11),
            b(12),
            if to_pay.cents >= 0 {
                "应缴"
            } else {
                "应退"
            },
            money(to_pay.abs()),
        )
    } else {
        format!(
            "GST period {} to {} ({} entries), due {}.\n\
             Box 5 sales and income incl. GST: {}\nBox 8 GST on sales: {}\n\
             Box 11 purchases and expenses incl. GST: {}\nBox 12 GST credit: {}\n\
             Box 15 {}: {}",
            r.period.start,
            r.period.end,
            r.entries,
            r.due,
            b(5),
            b(8),
            b(11),
            b(12),
            if to_pay.cents >= 0 {
                "to pay"
            } else {
                "refund"
            },
            money(to_pay.abs()),
        )
    };
    for w in &r.warnings {
        out.push_str(if zh { "\n注意：" } else { "\nNote: " });
        out.push_str(w);
    }
    out
}

/// An IR3 summary in words.
#[must_use]
pub fn ir3(r: &Ir3, zh: bool) -> String {
    let mut out = if zh {
        format!(
            "{} 税年（{} 笔分录），自行申报 {} 前截止。\n收入（不含 GST）：{}\n\
             费用（不含 GST）：{}\n利润：{}\n按税率表估算的所得税：{}",
            r.year,
            r.entries,
            r.due,
            money(r.income),
            money(r.expenses),
            money(r.profit),
            money(r.tax),
        )
    } else {
        format!(
            "Tax year {} ({} entries), self-filed return due {}.\nIncome excl. GST: {}\n\
             Expenses excl. GST: {}\nProfit: {}\nIncome tax on it, by the bands: {}",
            r.year,
            r.entries,
            r.due,
            money(r.income),
            money(r.expenses),
            money(r.profit),
            money(r.tax),
        )
    };
    for n in &r.notes {
        out.push_str(if zh { "\n注意：" } else { "\nNote: " });
        out.push_str(n);
    }
    out
}

/// What the agent can be asked.
#[must_use]
pub fn help(zh: bool) -> &'static str {
    if zh {
        "可以这样问我：\n扫描：检查读范围内还没看过的发票\n\
         GST：本期 GST 申报数字（\"上期 GST\" 看上一期）\n\
         所得税：本税年的收入、费用和估算税款\n待审：需要你看一眼的单据\n\
         频率 每月 / 两月 / 六个月：设置 GST 申报频率"
    } else {
        "Ask me:\nscan: look at invoices in scope I have not read yet\n\
         gst: this period's GST return (\"last gst\" for the one before)\n\
         income tax: this tax year's income, expenses and tax\n\
         review: documents that need your eye\n\
         frequency monthly / two-monthly / six-monthly: how often you file GST"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_the_way_people_write_them() {
        assert_eq!(money(Money::nzd(123_450)), "NZD 1,234.50");
        assert_eq!(money(Money::nzd(-1_200)), "-NZD 12.00");
        assert_eq!(money(Money::nzd(5)), "NZD 0.05");
        assert!(chinese("上期 GST"));
        assert!(!chinese("last gst"));
    }
}
