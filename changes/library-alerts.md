### Added

- Alerts for admins when a library stops responding and when it answers again, by webhook,
  by email, or both, set up under Outside services ("Alerts by webhook", "Alerts by email",
  each with a Test that sends a test alert). The webhook takes an ntfy topic as plain text
  with a title, or JSON for Gotify, Discord or Home Assistant (`MELETE_ALERT_WEBHOOK_URL`,
  `MELETE_ALERT_WEBHOOK_FORMAT`). Email goes through a mail server of your choosing, always
  encrypted (`MELETE_SMTP_HOST`, `MELETE_SMTP_PORT`, `MELETE_SMTP_SECURITY`,
  `MELETE_SMTP_USERNAME`, `MELETE_SMTP_PASSWORD`, `MELETE_ALERT_EMAIL_FROM`,
  `MELETE_ALERT_EMAIL_TO`). Both are off until set.
