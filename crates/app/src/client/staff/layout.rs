//! Persistent shell of the manager and admin pages: app bar with the three destinations and the
//! signed-in person. It is the layout route of the staff pages, so it stays mounted while the
//! content below it changes, and it loads the [`StaffContext`] once for all of them.

use dioxus::prelude::*;

use super::flow::SIGN_IN;
use super::load::{Skeleton, use_loaded};
use crate::api::auth::logout;
use crate::api::staff::{StaffContext, staff_context};
use crate::client::Route;
use crate::client::employee::use_org_name;
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

const STAFF_CSS: Asset = asset!("/assets/staff.css");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaffNav {
    Today,
    People,
    Settings,
}

impl StaffNav {
    fn label(self) -> String {
        match self {
            Self::Today => fl!(LANGUAGE_LOADER, "staff-nav-today"),
            Self::People => fl!(LANGUAGE_LOADER, "staff-nav-people"),
            Self::Settings => fl!(LANGUAGE_LOADER, "staff-nav-settings"),
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Today => "/beheer",
            Self::People => "/beheer/mensen",
            Self::Settings => "/beheer/instellingen",
        }
    }

    fn of(route: &Route) -> Option<Self> {
        match route {
            Route::StaffTodayPage {} => Some(Self::Today),
            Route::StaffPeoplePage { .. } => Some(Self::People),
            Route::StaffSettingsPage {} => Some(Self::Settings),
            _ => None,
        }
    }
}

/// Destinations for a role: "Instellingen" for admins only. Managers see their own sites in the
/// other two.
pub fn nav_items(is_admin: bool) -> Vec<StaffNav> {
    let mut items = vec![StaffNav::Today, StaffNav::People];
    if is_admin {
        items.push(StaffNav::Settings);
    }
    items
}

#[component]
pub fn StaffLayout() -> Element {
    let context = use_loaded(staff_context);
    let ctx = context.get();
    let active = StaffNav::of(&use_route::<Route>());
    let items = nav_items(ctx.as_ref().is_some_and(|c| c.is_admin));
    let org_name = use_org_name();
    let initial = org_name
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default();
    let first_name = ctx
        .as_ref()
        .map(|c| c.first_name.clone())
        .unwrap_or_default();
    let avatar = first_name
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default();
    rsx! {
        document::Stylesheet { href: STAFF_CSS }
        div { class: "st-shell",
            header { class: "st-appbar",
                span { class: "st-brand",
                    span { class: "st-brand-mark", aria_hidden: "true", "{initial}" }
                    span { class: "st-brand-name", "{org_name}" }
                }
                nav { class: "st-nav", aria_label: fl!(LANGUAGE_LOADER, "staff-nav-label"),
                    for item in items {
                        Link {
                            key: "{item.label()}",
                            to: item.path(),
                            class: if Some(item) == active { "st-tab st-tab-active" } else { "st-tab" },
                            "aria-current": if Some(item) == active { "page" } else { "false" },
                            {item.label()}
                        }
                    }
                }
                div { class: "st-account",
                    span { class: "st-person", "{first_name}" }
                    span { class: "st-avatar", aria_hidden: "true", "{avatar}" }
                    button {
                        class: "st-btn st-btn-text",
                        r#type: "button",
                        onclick: move |_| {
                            spawn(async move {
                                let _ = logout().await;
                                navigator().replace(SIGN_IN);
                            });
                        },
                        {fl!(LANGUAGE_LOADER, "staff-logout")}
                    }
                }
            }
            main { class: "st-main",
                match ctx {
                    Some(ctx) => rsx! {
                        ContextScope { ctx, Outlet::<Route> {} }
                    },
                    None if context.failed() => rsx! {
                        p { class: "st-sub st-page", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                    },
                    None => rsx! { Skeleton { rows: 6 } },
                }
            }
        }
    }
}

/// Provides the loaded staff context to the pages below.
#[component]
fn ContextScope(ctx: StaffContext, children: Element) -> Element {
    use_context_provider(|| ctx);
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_sees_today_and_people() {
        assert_eq!(nav_items(false), vec![StaffNav::Today, StaffNav::People]);
    }

    #[test]
    fn admin_also_sees_settings() {
        assert_eq!(
            nav_items(true),
            vec![StaffNav::Today, StaffNav::People, StaffNav::Settings]
        );
    }
}
