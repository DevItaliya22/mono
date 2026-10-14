// SPDX-License-Identifier: MPL-2.0

//! The email journey's own screens: the code, and the password.
//!
//! The welcome screen is [`koru_ui::welcome`]'s, extended there so the two
//! fields sit in the same measured column as the buttons. These two are this
//! form's, drawn in the same content column as the Profile and Workspace
//! steps, because that is what the rest of first run looks like.
//!
//! **What the worker decides, and what this decides.** Which journey an
//! address and a password turn out to be is the worker's — one command leaves
//! this form and the stage that comes back says which screen to show. What is
//! here is only what a person sees: six boxes, a rule said before it is
//! needed, and the handful of failures a screen answers itself rather than
//! throwing somebody out to an error page.
use super::*;

/// Six digits (D11), and the boxes that hold them.
pub(super) const CODE_LEN: usize = 6;
const BOX_W: f32 = 48.0;
const BOX_GAP: f32 = 12.0;
const BOX_H: f32 = 56.0;
const FIELD_H: f32 = 44.0;

impl OnboardingView {
    // ------------------------------------------------------------- geometry

    /// The row of digit boxes, as one rectangle. Hit-testing takes the row and
    /// not a box: every box edits the same value, so there is nothing to
    /// choose between them.
    pub(super) fn email_code_row(&self) -> Rect {
        let c = self.content();
        let w = CODE_LEN as f32 * BOX_W + (CODE_LEN - 1) as f32 * BOX_GAP;
        Rect::new(c.x + (c.w - w) * 0.5, c.y + 232.0, w, BOX_H)
    }

    fn email_code_box(&self, index: usize) -> Rect {
        let row = self.email_code_row();
        Rect::new(
            row.x + index as f32 * (BOX_W + BOX_GAP),
            row.y,
            BOX_W,
            BOX_H,
        )
    }

    /// *Send another code* and *Use a different address*, side by side under
    /// the boxes. Both are quiet: neither is the way forward.
    pub(super) fn email_code_actions(&mut self) -> (Rect, Rect) {
        let row = self.email_code_row();
        let resend_w =
            ActionButton::new(RESEND_LABEL, ActionStyle::Ghost, None).width(&mut self.engine);
        let another_w =
            ActionButton::new(ANOTHER_LABEL, ActionStyle::Ghost, None).width(&mut self.engine);
        let total = resend_w + BUTTON_GAP + another_w;
        let c = self.content();
        let x = c.x + (c.w - total) * 0.5;
        let y = row.y + row.h + 24.0;
        (
            Rect::new(x, y, resend_w, 36.0),
            Rect::new(x + resend_w + BUTTON_GAP, y, another_w, 36.0),
        )
    }

    pub(super) fn new_password_rect(&self) -> Rect {
        let c = self.content();
        Rect::new(c.x, c.y + 210.0, c.w, FIELD_H)
    }

    pub(super) fn confirm_password_rect(&self) -> Rect {
        let field = self.new_password_rect();
        Rect::new(field.x, field.y + field.h + 36.0, field.w, FIELD_H)
    }

    // -------------------------------------------------------------- opening

    pub(super) fn show_email_code(&mut self) {
        self.screen = Screen::EmailCode;
        self.focus = 0;
        self.email_code.set_text(&mut self.engine, "");
    }

    pub(super) fn show_email_password(&mut self) {
        self.screen = Screen::EmailPassword;
        self.focus = 0;
        self.new_password.set_text(&mut self.engine, "");
        self.confirm_password.set_text(&mut self.engine, "");
        self.error = None;
    }

    /// Back to the welcome screen, with the reason when there is one.
    ///
    /// The address is kept and the password is not: everything that ends a
    /// journey this way ends it because the password was wrong, absent, or
    /// beside the point. `None` is somebody choosing to go back, which needs
    /// no explaining.
    pub(super) fn abandon_email_journey(&mut self, message: Option<&str>) {
        self.email_password.set_text(&mut self.engine, "");
        self.email_password.conceal();
        self.email_code.set_text(&mut self.engine, "");
        self.new_password.set_text(&mut self.engine, "");
        self.confirm_password.set_text(&mut self.engine, "");
        self.browser_signin = None;
        self.screen = Screen::Entry;
        self.focus = 0;
        self.error = message.map(ToOwned::to_owned);
        self.error_field = None;
    }

    // ------------------------------------------------------------- failures

    /// **The failures an email screen answers itself.**
    ///
    /// Everything else is a real failure and goes where every other failure
    /// goes, to the account-status screen. Without this a mistyped digit would
    /// throw somebody off the code screen onto an error page, and the code
    /// they are holding would be unusable.
    ///
    /// Returns `true` when it has taken charge of the state.
    pub(super) fn settle_email_failure(&mut self, state: &AccountState) -> bool {
        if state.stage != koru_agent_wire::AccountStage::Failed {
            return false;
        }
        if !matches!(
            self.screen,
            Screen::EmailWorking | Screen::EmailCode | Screen::EmailPassword
        ) {
            return false;
        }
        let Some(code) = state.failure_code.as_deref() else {
            return false;
        };
        // Ends the journey: the flow is spent, or the address is not ours to
        // sign up. Said on the welcome screen, which is where the way forward
        // is.
        if let Some(message) = match code {
            "email_code_attempts_exhausted" => {
                Some("Too many attempts. Start again to get a new code.")
            }
            // **The one place Koru says an address is taken**, and it is said
            // only to somebody who has just proved they read that mailbox.
            "email_already_registered" => Some(
                "You already have a Koru account with this address, and that password did not \
                 match. Try again.",
            ),
            "email_already_linked_to_google" => {
                Some("This address already signs in with Google. Use the Google button.")
            }
            "email_address_taken" => {
                Some("An account was just created with this address. Try signing in.")
            }
            "email_journey_absent" | "email_code_unproved" => {
                Some("That sign-in expired. Start again.")
            }
            _ => None,
        } {
            self.abandon_email_journey(Some(message));
            // The password that did not match is the one thing to fix; every
            // other ending is about the attempt.
            if code == "email_already_registered" {
                self.error_field = Some(welcome::Action::Password);
            }
            return true;
        }
        // Stays where it is: the screen is still the way forward.
        let Some(message) = (match code {
            "email_code_incorrect" => {
                Some("That code is not right. Check the email and try again.")
            }
            "email_code_expired" => Some("That code has expired. Ask for a new one."),
            "email_code_invalid" => Some("A code is six digits."),
            "email_resend_too_soon" => Some("You can ask for another code in a moment."),
            "email_password_refused" => Some(PASSWORD_RULE),
            "email_password_invalid" => Some("Type a password."),
            "email_address_invalid" => Some("That does not look like an email address."),
            _ => None,
        }) else {
            return false;
        };
        // A failure while the worker was still thinking belongs on the screen
        // that asked, which is the one before this.
        if self.screen == Screen::EmailWorking {
            self.screen = Screen::Entry;
            self.focus = 0;
        }
        self.error = Some(message.to_owned());
        // On the welcome screen these two are about a field, not the attempt.
        self.error_field = match code {
            "email_address_invalid" => Some(welcome::Action::Address),
            "email_password_invalid" => Some(welcome::Action::Password),
            _ => None,
        };
        true
    }

    // ------------------------------------------------------------ the keys

    /// Six digits and nothing else.
    ///
    /// The editor underneath is a full [`TextInput`] so that paste, select-all
    /// and undo work the way they do in every other field — a code arrives in
    /// a mail client and is pasted at least as often as it is typed. What it
    /// holds is then trimmed to what six boxes can show.
    pub(super) fn email_code_typed(
        &mut self,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Typed {
        let typed = self.email_code.key(&mut self.engine, key, text, mods);
        if typed != Typed::Handled {
            return typed;
        }
        let value = self.email_code.value();
        let digits: String = value
            .chars()
            .filter(|c| c.is_ascii_digit())
            .take(CODE_LEN)
            .collect();
        if digits != value {
            self.email_code.set_text(&mut self.engine, &digits);
        }
        Typed::Handled
    }

    pub(super) fn email_code_complete(&self) -> bool {
        self.email_code.value().chars().count() == CODE_LEN
    }

    /// Both boxes filled and the same. The rule itself is the server's — this
    /// only refuses to send two different passwords, which no server can
    /// notice on its own.
    pub(super) fn new_password_ready(&self) -> bool {
        !self.new_password.is_empty() && self.new_password.value() == self.confirm_password.value()
    }

    // ------------------------------------------------------------- drawing

    pub(super) fn draw_email_code(&mut self, canvas: &mut Scene, root: Affine) {
        let p = theme::p();
        let c = self.content();
        draw_text(
            &mut self.engine,
            canvas,
            root,
            "Check your email",
            c.x,
            c.y + 108.0,
            theme::role::subtitle(),
            "theme::role::subtitle()",
            p.text,
        );
        let blurb = format!(
            "We sent a six-digit code to {}. It expires in 10 minutes.",
            self.email_address.value()
        );
        draw_wrapped_text(
            &mut self.engine,
            canvas,
            root,
            &blurb,
            c.x,
            c.y + 156.0,
            c.w,
            theme::role::row(),
            "theme::role::row()",
            p.text_dim,
        );

        let focused = self.controls().get(self.focus).copied();
        let digits: Vec<char> = self.email_code.value().chars().collect();
        let caret = digits.len().min(CODE_LEN - 1);
        let row = self.email_code_row();
        let _boxes = inspect::scope(
            "CodeBoxes",
            "component",
            root,
            scene::rect(row.x, row.y, row.w, row.h),
            |record| {
                record.property("control", "six digits", "koru_ui::text_input");
                record.property("filled", digits.len() as u64, "view state");
            },
        );
        for index in 0..CODE_LEN {
            let r = self.email_code_box(index);
            // The ring sits on the box the next digit lands in, so the boxes
            // read as one field rather than six.
            let lit = focused == Some(Hit::EmailCode) && index == caret;
            scene::fill(
                canvas,
                root,
                p.panel_bg,
                &scene::rrect(r.x, r.y, r.w, r.h, 8.0),
            );
            scene::stroke(
                canvas,
                root,
                if lit { p.accent } else { p.panel_border },
                theme::BORDER_W,
                &scene::rrect(r.x, r.y, r.w, r.h, 8.0),
            );
            let Some(digit) = digits.get(index) else {
                continue;
            };
            let glyph = digit.to_string();
            let layout = self.engine.simple_layout(&glyph, theme::role::subtitle());
            draw_text(
                &mut self.engine,
                canvas,
                root,
                &glyph,
                r.x + (r.w - layout.width()) * 0.5,
                r.y + (r.h - scene::first_line_height(&layout)) * 0.5,
                theme::role::subtitle(),
                "theme::role::subtitle()",
                p.text,
            );
        }

        let (resend, another) = self.email_code_actions();
        ActionButton::new(RESEND_LABEL, ActionStyle::Ghost, None).draw_focused(
            canvas,
            root,
            &mut self.engine,
            resend,
            self.hover == Some(Hit::ResendCode),
            focused == Some(Hit::ResendCode),
        );
        ActionButton::new(ANOTHER_LABEL, ActionStyle::Ghost, None).draw_focused(
            canvas,
            root,
            &mut self.engine,
            another,
            self.hover == Some(Hit::AnotherAddress),
            focused == Some(Hit::AnotherAddress),
        );
        let ready = self.email_code_complete();
        let primary = self.button("Continue", 64.0);
        draw_button_enabled(
            canvas,
            root,
            &mut self.engine,
            primary,
            "Continue",
            ButtonRole::Primary,
            self.hover == Some(Hit::Primary),
            focused == Some(Hit::Primary),
            ready,
        );
        self.draw_error(canvas, root, primary.y - 44.0);
    }

    pub(super) fn draw_email_password(&mut self, canvas: &mut Scene, root: Affine) {
        let p = theme::p();
        let c = self.content();
        draw_text(
            &mut self.engine,
            canvas,
            root,
            "Choose a password",
            c.x,
            c.y + 108.0,
            theme::role::subtitle(),
            "theme::role::subtitle()",
            p.text,
        );
        // Before anything is typed, never as a rejection afterwards.
        draw_wrapped_text(
            &mut self.engine,
            canvas,
            root,
            PASSWORD_RULE,
            c.x,
            c.y + 156.0,
            c.w,
            theme::role::row(),
            "theme::role::row()",
            p.text_dim,
        );

        let focused = self.controls().get(self.focus).copied();
        for (hit, rect, label) in [
            (Hit::NewPassword, self.new_password_rect(), "Password"),
            (
                Hit::ConfirmPassword,
                self.confirm_password_rect(),
                "Confirm password",
            ),
        ] {
            draw_text(
                &mut self.engine,
                canvas,
                root,
                label,
                rect.x,
                rect.y - 22.0,
                theme::role::meta(),
                "theme::role::meta()",
                p.text_dim,
            );
            let lit = focused == Some(hit);
            let style = InputStyle::form(lit).caret_shown(self.caret_on());
            let field = if hit == Hit::NewPassword {
                &mut self.new_password
            } else {
                &mut self.confirm_password
            };
            let _scope = inspect::scope(
                "PasswordField",
                "component",
                root,
                scene::rect(rect.x, rect.y, rect.w, rect.h),
                |record| {
                    record.property("control", "TextInput", "koru_ui::text_input");
                    record.property("masked", true, "view state");
                    record.property("focused", lit, "view state");
                },
            );
            field.draw_masked(canvas, root, &mut self.engine, rect, style, None, lit);
        }

        let ready = self.new_password_ready();
        let primary = self.button(CREATE_ACCOUNT_LABEL, 64.0);
        draw_button_enabled(
            canvas,
            root,
            &mut self.engine,
            primary,
            CREATE_ACCOUNT_LABEL,
            ButtonRole::Primary,
            self.hover == Some(Hit::Primary),
            focused == Some(Hit::Primary),
            ready,
        );
        self.draw_error(canvas, root, primary.y - 44.0);
    }

    /// The failure line these two screens share, drawn where both leave room
    /// for it: above the action, under the form.
    fn draw_error(&mut self, canvas: &mut Scene, root: Affine, y: f32) {
        let Some(message) = self.error.clone() else {
            return;
        };
        let c = self.content();
        draw_wrapped_text(
            &mut self.engine,
            canvas,
            root,
            &message,
            c.x,
            y,
            c.w,
            theme::role::meta(),
            "theme::role::meta()",
            theme::p().notice_fg,
        );
    }
}

/// The two quiet actions on the code screen, and the primary on the password
/// screen. Literals rather than rows in the string table, like every other
/// drawn word on this route — see the note on `A11yFirstRun*` in `koru-i18n`.
pub(super) const RESEND_LABEL: &str = "Send another code";
pub(super) const ANOTHER_LABEL: &str = "Use a different address";
pub(super) const CREATE_ACCOUNT_LABEL: &str = "Create account";
/// D9, and the same sentence the website shows.
pub(super) const PASSWORD_RULE: &str =
    "At least 8 characters, using at least three of: capitals, lowercase, digits, symbols.";
