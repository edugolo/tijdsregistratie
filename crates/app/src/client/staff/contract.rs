//! "Contract" of the person panel (HR and admins): contract time per weekday and the payroll
//! number, which stays hidden until it is asked for.

use dioxus::prelude::*;
use uuid::Uuid;

use super::field::{Field, error_id};
use super::flow::{ErrorNotice, error_code, report_error};
use super::hours_fmt::{format_duration, format_hhmm, parse_hhmm};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use crate::api::hours::{ContractDto, person_contract, set_contract, set_payroll_number};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// Longest payroll number, in characters (the server refuses more).
const PAYROLL_NUMBER_MAX_CHARS: usize = 20;

fn weekday_label(index: usize) -> String {
    match index {
        0 => fl!(LANGUAGE_LOADER, "weekday-mon"),
        1 => fl!(LANGUAGE_LOADER, "weekday-tue"),
        2 => fl!(LANGUAGE_LOADER, "weekday-wed"),
        3 => fl!(LANGUAGE_LOADER, "weekday-thu"),
        4 => fl!(LANGUAGE_LOADER, "weekday-fri"),
        5 => fl!(LANGUAGE_LOADER, "weekday-sat"),
        _ => fl!(LANGUAGE_LOADER, "weekday-sun"),
    }
}

/// Every weekday as typed. Empty counts as no contract time; `None` when one is not valid.
fn parse_week(texts: &[String; 7]) -> Option<[u32; 7]> {
    let mut week = [0; 7];
    for (slot, text) in week.iter_mut().zip(texts) {
        *slot = if text.trim().is_empty() {
            0
        } else {
            parse_hhmm(text)?
        };
    }
    Some(week)
}

/// The payroll number to store: trimmed, `None` clears it. `Err` when it is too long.
fn check_payroll_number(text: &str) -> Result<Option<String>, ()> {
    let text = text.trim();
    if text.chars().count() > PAYROLL_NUMBER_MAX_CHARS || text.chars().any(char::is_control) {
        return Err(());
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

#[component]
pub fn ContractSection(person_id: Uuid) -> Element {
    let loaded = use_loaded(move || person_contract(person_id));
    rsx! {
        section { class: "st-panel-section st-stretch",
            h3 { {fl!(LANGUAGE_LOADER, "contract-title")} }
            match loaded.get() {
                Some(contract) => rsx! { ContractForm { person_id, contract } },
                None if loaded.failed() => rsx! {
                    p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                },
                None => rsx! { Skeleton { rows: 3 } },
            }
        }
    }
}

#[component]
fn ContractForm(person_id: Uuid, contract: ContractDto) -> Element {
    let week = contract.minutes_per_weekday;
    let mut texts = use_signal(|| week.map(format_hhmm));
    let mut payroll = use_signal(|| contract.payroll_number.clone().unwrap_or_default());
    let mut revealed = use_signal(|| false);
    let mut week_problem = use_signal(|| false);
    let mut payroll_problem = use_signal(|| None::<String>);
    let mut saved = use_signal(|| None::<&'static str>);
    let message = use_signal(|| None::<String>);
    let total = parse_week(&texts.read()).map(|w| w.iter().sum::<u32>());
    rsx! {
        ErrorNotice { message }
        form {
            class: "st-form",
            novalidate: true,
            aria_label: fl!(LANGUAGE_LOADER, "contract-week"),
            onsubmit: move |e| {
                e.prevent_default();
                saved.set(None);
                let Some(minutes) = parse_week(&texts.read()) else {
                    week_problem.set(true);
                    return;
                };
                week_problem.set(false);
                spawn(async move {
                    match set_contract(person_id, minutes).await {
                        Ok(()) => saved.set(Some("contract")),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            div { class: "st-contract-grid",
                for index in 0..7usize {
                    div { key: "{index}", class: "st-field st-field-day",
                        label { r#for: "contract-day-{index}", {weekday_label(index)} }
                        input {
                            id: "contract-day-{index}",
                            class: "st-input",
                            r#type: "text",
                            inputmode: "numeric",
                            autocomplete: "off",
                            placeholder: "0:00",
                            aria_invalid: week_problem() && parse_hhmm(&texts.read()[index]).is_none() && !texts.read()[index].trim().is_empty(),
                            value: "{texts.read()[index]}",
                            oninput: move |e| texts.write()[index] = e.value(),
                        }
                    }
                }
            }
            if week_problem() {
                span { class: "st-error", role: "alert",
                    Icon { kind: IconKind::Alert, size: 14 }
                    {fl!(LANGUAGE_LOADER, "contract-err-time")}
                }
            }
            if let Some(total) = total {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "contract-total", time = format_duration(total))} }
            }
            div { class: "st-inline",
                button { class: "st-btn st-btn-tonal", r#type: "submit", {fl!(LANGUAGE_LOADER, "contract-save")} }
                if saved() == Some("contract") {
                    span { class: "st-sub", role: "status", {fl!(LANGUAGE_LOADER, "hours-saved")} }
                }
            }
        }
        form {
            class: "st-form",
            novalidate: true,
            onsubmit: move |e| {
                e.prevent_default();
                saved.set(None);
                let Ok(number) = check_payroll_number(&payroll.read()) else {
                    payroll_problem.set(Some(fl!(LANGUAGE_LOADER, "contract-err-payroll")));
                    return;
                };
                payroll_problem.set(None);
                spawn(async move {
                    let mut payroll_problem = payroll_problem;
                    match set_payroll_number(person_id, number).await {
                        Ok(()) => saved.set(Some("payroll")),
                        Err(e) if error_code(&e) == Some(409) => {
                            payroll_problem.set(Some(fl!(LANGUAGE_LOADER, "contract-err-taken")));
                        }
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            Field {
                control_id: "payroll-number",
                label: fl!(LANGUAGE_LOADER, "contract-payroll"),
                error: payroll_problem(),
                div { class: "st-inline st-reveal",
                    input {
                        id: "payroll-number",
                        class: "st-input",
                        r#type: if revealed() { "text" } else { "password" },
                        autocomplete: "off",
                        spellcheck: "false",
                        aria_invalid: payroll_problem().is_some(),
                        aria_describedby: payroll_problem().is_some().then(|| error_id("payroll-number")),
                        value: "{payroll}",
                        oninput: move |e| payroll.set(e.value()),
                    }
                    button {
                        class: "st-icon-btn",
                        r#type: "button",
                        aria_pressed: "{revealed}",
                        aria_label: if revealed() {
                            fl!(LANGUAGE_LOADER, "contract-hide")
                        } else {
                            fl!(LANGUAGE_LOADER, "contract-show")
                        },
                        onclick: move |_| revealed.set(!revealed()),
                        Icon {
                            kind: if revealed() { IconKind::EyeOff } else { IconKind::Eye },
                            size: 20,
                        }
                    }
                }
            }
            div { class: "st-inline",
                button { class: "st-btn st-btn-tonal", r#type: "submit", {fl!(LANGUAGE_LOADER, "contract-payroll-save")} }
                if saved() == Some("payroll") {
                    span { class: "st-sub", role: "status", {fl!(LANGUAGE_LOADER, "hours-saved")} }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(values: [&str; 7]) -> [String; 7] {
        values.map(String::from)
    }

    #[test]
    fn a_week_of_times_becomes_minutes() {
        assert_eq!(
            parse_week(&texts(["7:36", "7:36", "3:48", "7:36", "7:36", "", "0:00"])),
            Some([456, 456, 228, 456, 456, 0, 0])
        );
    }

    #[test]
    fn one_wrong_day_refuses_the_week() {
        assert_eq!(parse_week(&texts(["7:36", "x", "", "", "", "", ""])), None);
        assert_eq!(parse_week(&texts(["", "", "", "", "", "", "24:01"])), None);
    }

    #[test]
    fn a_payroll_number_is_trimmed_and_may_be_cleared() {
        assert_eq!(check_payroll_number(" P-100 "), Ok(Some("P-100".into())));
        assert_eq!(check_payroll_number("   "), Ok(None));
        assert_eq!(
            check_payroll_number(&"1".repeat(20)).map(|n| n.is_some()),
            Ok(true)
        );
        assert_eq!(check_payroll_number(&"1".repeat(21)), Err(()));
        assert_eq!(check_payroll_number("a\tb"), Err(()));
    }
}
