use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Row, Sparkline, Table, Tabs,
        Wrap,
    },
    Frame,
};

use super::app::{App, AppMode, Tab};

const SELECTED_STYLE: Style = Style::new();
const DIM_STYLE:      Style = Style::new().fg(Color::DarkGray);
const OK_STYLE:       Style = Style::new().fg(Color::Green);
const ERR_STYLE:      Style = Style::new().fg(Color::Red);
const HDR_STYLE:      Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const WARN_STYLE:     Style = Style::new().fg(Color::Yellow);

pub fn draw(frame: &mut Frame, app: &mut App) {
    if matches!(app.mode, AppMode::Login) {
        draw_login(frame, app);
        return;
    }

    let area = frame.area();
    let [title_area, body_area, status_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ]).areas(area);

    draw_tabs(frame, title_area, app);
    draw_body(frame, body_area, app);
    draw_status(frame, status_area, app);

    // Overlays (highest priority last so they render on top)
    if app.new_tok_value.is_some() { draw_new_token(frame, area, app); }
    if app.confirm.is_some()       { draw_confirm(frame, area, app); }
    if app.publish.is_some()       { draw_publish_form(frame, area, app); }
    if app.tok_create.is_some()    { draw_create_token_form(frame, area, app); }
    if app.org_create.is_some()    { draw_create_org_form(frame, area, app); }
    if app.add_member.is_some()    { draw_add_member_form(frame, area, app); }
    if app.add_owner.is_some()     { draw_add_owner_form(frame, area, app); }
}

// ── Tabs bar ──────────────────────────────────────────────────────────────────

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles = vec!["1:Packages", "2:Users", "3:Tokens", "4:Orgs", "5:Audit"];
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .block(Block::default()
            .title(format!(
                " freight-registry — {}{}",
                app.current_user,
                if app.is_admin { " [admin]" } else { "" }
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded))
        .style(Style::default().fg(Color::Reset))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .divider("│");
    frame.render_widget(tabs, area);
}

// ── Body dispatch ─────────────────────────────────────────────────────────────

fn draw_body(frame: &mut Frame, area: Rect, app: &mut App) {
    match app.tab {
        Tab::Packages => draw_packages(frame, area, app),
        Tab::Users    => draw_users(frame, area, app),
        Tab::Tokens   => draw_tokens(frame, area, app),
        Tab::Orgs     => draw_orgs(frame, area, app),
        Tab::Audit    => draw_audit(frame, area, app),
    }
}

// ── Packages tab ──────────────────────────────────────────────────────────────

fn draw_packages(frame: &mut Frame, area: Rect, app: &mut App) {
    let [search_area, list_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
    ]).areas(area);

    let search_block = Block::default()
        .title(if app.pkg_search_on { " Search (Enter to run, Esc to cancel) " }
               else { " / to search  r to refresh  P to publish " })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.pkg_search_on { Style::default().fg(Color::Yellow) }
                      else { Style::default() });
    frame.render_widget(Paragraph::new(app.pkg_search.as_str()).block(search_block), search_area);

    if app.pkg_detail.is_some() {
        let [left, right] = Layout::horizontal([
            Constraint::Percentage(35),
            Constraint::Percentage(65),
        ]).areas(list_area);
        draw_package_list(frame, left, app);
        draw_package_detail(frame, right, app);
    } else {
        draw_package_list(frame, list_area, app);
    }
}

fn draw_package_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let loading_sfx = if app.loading { " (loading…)" } else { "" };
    let items: Vec<ListItem> = app.packages.iter().map(|p| {
        let ver  = p.latest.as_deref().unwrap_or("?");
        let dl   = p.downloads;
        ListItem::new(Line::from(vec![
            Span::raw(format!("{:<28}", &p.name)),
            Span::styled(format!(" {:<10}", ver), DIM_STYLE),
            Span::styled(format!(" ↓{dl}"), DIM_STYLE),
        ]))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .title(format!(" Packages ({}){} ", app.packages.len(), loading_sfx))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded))
        .highlight_style(SELECTED_STYLE)
        .highlight_symbol("► ");

    frame.render_stateful_widget(list, area, &mut app.pkg_state);
}

fn draw_package_detail(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(detail) = &app.pkg_detail else { return };

    let has_spark  = detail.versions.len() >= 2;
    let spark_h    = if has_spark { 3 } else { 0 };

    let [info_area, ver_area, spark_area, hint_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(4),
        Constraint::Length(spark_h),
        Constraint::Length(1),
    ]).areas(area);

    // Info block — name, description, owners
    let owners_str = if detail.owners.is_empty() { "—".to_string() } else { detail.owners.join(", ") };
    let desc       = detail.description.as_deref().unwrap_or("—");
    let info = Paragraph::new(vec![
        Line::from(vec![Span::styled("Name:  ", HDR_STYLE), Span::raw(&detail.name)]),
        Line::from(vec![Span::styled("Desc:  ", HDR_STYLE), Span::raw(desc)]),
        Line::from(vec![Span::styled("Owners:", HDR_STYLE), Span::raw(format!(" {owners_str}"))]),
    ])
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Detail "));
    frame.render_widget(info, info_area);

    // Versions list
    let items: Vec<ListItem> = detail.versions.iter().map(|v| {
        let tag  = if v.yanked { Span::styled(" [yanked]", ERR_STYLE) }
                   else        { Span::styled(" [active]", OK_STYLE) };
        let pb   = if v.prebuilt_triples.is_empty() { String::new() }
                   else { format!(" 📦{}", v.prebuilt_triples.len()) };
        ListItem::new(Line::from(vec![
            Span::raw(format!("{:<14}", v.version)),
            Span::styled(format!("↓{:<8}", v.downloads), DIM_STYLE),
            tag,
            Span::styled(pb, Style::default().fg(Color::Magenta)),
        ]))
    }).collect();

    let ver_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
            .title(" Versions (j/k  y=yank  u=unyank  📦=has prebuilts) "))
        .highlight_style(SELECTED_STYLE)
        .highlight_symbol("► ");
    frame.render_stateful_widget(ver_list, ver_area, &mut app.ver_state);

    if has_spark {
        let spark_data: Vec<u64> = detail.versions.iter().rev()
            .map(|v| v.downloads.max(0) as u64)
            .collect();
        let sparkline = Sparkline::default()
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                .title(" Downloads (oldest → newest) "))
            .data(&spark_data)
            .style(Style::default().fg(Color::Cyan));
        frame.render_widget(sparkline, spark_area);
    }

    let mut hints = vec![
        Span::raw(" [Esc] back  [y] yank  [u] unyank  [a] add owner  [O] remove owner"),
    ];
    if app.is_admin { hints.push(Span::styled("  [d] delete pkg", ERR_STYLE)); }
    frame.render_widget(Paragraph::new(Line::from(hints)), hint_area);
}

// ── Users tab ─────────────────────────────────────────────────────────────────

fn draw_users(frame: &mut Frame, area: Rect, app: &mut App) {
    let header = Row::new(["ID", "Username", "Email", "Admin"]).style(HDR_STYLE);
    let rows: Vec<Row> = app.users.iter().map(|u| {
        let admin = if u.is_admin { "✓" } else { "" };
        Row::new(vec![
            u.id.to_string(),
            u.username.clone(),
            u.email.clone().unwrap_or_default(),
            admin.to_string(),
        ])
    }).collect();

    let hint = if app.is_admin { " [p] promote  [d] demote  [x] remove  [r] refresh" }
               else            { " [r] refresh (admin access required for mutations)" };

    let [table_area, hint_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ]).areas(area);

    let table = Table::new(rows, [
        Constraint::Length(6),
        Constraint::Length(24),
        Constraint::Min(28),
        Constraint::Length(6),
    ])
    .header(header)
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" Users ({}) ", app.users.len())))
    .row_highlight_style(SELECTED_STYLE)
    .highlight_symbol("► ");

    frame.render_stateful_widget(table, table_area, &mut app.usr_state);
    frame.render_widget(Paragraph::new(hint), hint_area);
}

// ── Tokens tab ────────────────────────────────────────────────────────────────

fn draw_tokens(frame: &mut Frame, area: Rect, app: &mut App) {
    let header = Row::new(["ID", "Name", "Scope", "Expires", "Last used"]).style(HDR_STYLE);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let rows: Vec<Row> = app.tokens.iter().map(|t| {
        let expires = t.expires_at.map(|ts| {
            let days = (ts - now) / 86_400;
            if days < 0 { "expired".to_string() } else { format!("{days}d") }
        }).unwrap_or_else(|| "never".to_string());
        let last = t.last_used.map(|ts| {
            let secs = now - ts;
            if secs < 3600 { format!("{}m ago", secs / 60) }
            else           { format!("{}h ago", secs / 3600) }
        }).unwrap_or_else(|| "never".to_string());
        Row::new(vec![t.id.to_string(), t.name.clone(), t.scope.clone(), expires, last])
    }).collect();

    let [table_area, hint_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ]).areas(area);

    let table = Table::new(rows, [
        Constraint::Length(6),
        Constraint::Length(22),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Min(12),
    ])
    .header(header)
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" My Tokens ({}) ", app.tokens.len())))
    .row_highlight_style(SELECTED_STYLE)
    .highlight_symbol("► ");

    frame.render_stateful_widget(table, table_area, &mut app.tok_state);
    frame.render_widget(
        Paragraph::new(" [n] new token  [x/Del] revoke  [r] refresh"),
        hint_area,
    );
}

// ── Orgs tab ──────────────────────────────────────────────────────────────────

fn draw_orgs(frame: &mut Frame, area: Rect, app: &mut App) {
    let [body_area, hint_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ]).areas(area);

    if app.org_detail.is_some() {
        let [left, right] = Layout::horizontal([
            Constraint::Percentage(35),
            Constraint::Percentage(65),
        ]).areas(body_area);
        draw_org_list(frame, left, app);
        draw_org_detail(frame, right, app);
    } else {
        draw_org_list(frame, body_area, app);
    }

    let hint = if app.org_detail.is_some() {
        " [Esc] back  [a] add member  [x] remove member  [d] delete org  [r] refresh"
    } else {
        " [n] new org  [d] delete org  [r] refresh  [Enter/↑↓] select"
    };
    frame.render_widget(Paragraph::new(hint).style(DIM_STYLE), hint_area);
}

fn draw_org_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let loading_sfx = if app.loading { " (loading…)" } else { "" };
    let items: Vec<ListItem> = app.orgs.iter().map(|o| {
        let desc = o.description.as_deref().unwrap_or("");
        ListItem::new(Line::from(vec![
            Span::raw(format!("{:<24}", &o.name)),
            Span::styled(format!(" {}", desc), DIM_STYLE),
        ]))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .title(format!(" Organizations ({}){} ", app.orgs.len(), loading_sfx))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded))
        .highlight_style(SELECTED_STYLE)
        .highlight_symbol("► ");

    frame.render_stateful_widget(list, area, &mut app.org_state);
}

fn draw_org_detail(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(org) = &app.org_detail else { return };

    let [info_area, members_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(0),
    ]).areas(area);

    let desc = org.description.as_deref().unwrap_or("—");
    let info = Paragraph::new(vec![
        Line::from(vec![Span::styled("Name: ", HDR_STYLE), Span::raw(&org.name)]),
        Line::from(vec![Span::styled("Desc: ", HDR_STYLE), Span::raw(desc)]),
    ])
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Org "));
    frame.render_widget(info, info_area);

    let header = Row::new(["Username", "Role"]).style(HDR_STYLE);
    let rows: Vec<Row> = app.org_members.iter().map(|m| {
        let role_style = if m.role == "owner" { WARN_STYLE } else { Style::default() };
        Row::new(vec![m.username.clone(), m.role.clone()]).style(role_style)
    }).collect();

    let table = Table::new(rows, [Constraint::Min(20), Constraint::Length(8)])
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
            .title(format!(" Members ({}) ", app.org_members.len())))
        .row_highlight_style(SELECTED_STYLE)
        .highlight_symbol("► ");

    frame.render_stateful_widget(table, members_area, &mut app.org_mem_state);
}

// ── Audit tab ─────────────────────────────────────────────────────────────────

fn draw_audit(frame: &mut Frame, area: Rect, app: &mut App) {
    let [filter_area, table_area, hint_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ]).areas(area);

    let filter_block = Block::default()
        .title(if app.aud_filter_on { " Filter (user:name or action, Enter/Esc to apply) " }
               else { " / to filter  r to refresh " })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.aud_filter_on { Style::default().fg(Color::Yellow) }
                      else { Style::default() });
    frame.render_widget(
        Paragraph::new(app.aud_filter.as_str()).block(filter_block),
        filter_area,
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let header = Row::new(["Time", "User", "Action", "Package", "Version", "IP"])
        .style(HDR_STYLE);

    let rows: Vec<Row> = app.audit.iter().map(|e| {
        let age  = now - e.created_at;
        let time = if age < 3600 { format!("{}m ago", age / 60) }
                   else          { format!("{}h ago", age / 3600) };
        Row::new(vec![
            time,
            e.username.clone().unwrap_or_else(|| "—".into()),
            e.action.clone(),
            e.package.clone().unwrap_or_default(),
            e.version.clone().unwrap_or_default(),
            e.ip_addr.clone().unwrap_or_default(),
        ])
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(18),
        Constraint::Length(20),
        Constraint::Length(10),
        Constraint::Min(12),
    ])
    .header(header)
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" Audit Log ({}) ", app.audit.len())))
    .row_highlight_style(SELECTED_STYLE)
    .highlight_symbol("► ");

    frame.render_stateful_widget(table, table_area, &mut app.aud_state);
    frame.render_widget(Paragraph::new(" j/k or ↑↓ to scroll").style(DIM_STYLE), hint_area);
}

// ── Status bar ────────────────────────────────────────────────────────────────

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let spinner = if app.loading { "⣷ " } else { "" };
    let style   = if app.is_err { ERR_STYLE } else { OK_STYLE };
    let line    = Line::from(vec![
        Span::styled(spinner, Style::default().fg(Color::Yellow)),
        Span::styled(&app.status, style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ── Login screen ──────────────────────────────────────────────────────────────

fn draw_login(frame: &mut Frame, app: &App) {
    let area = center_rect(50, 14, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" freight-registry — login ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [url_a, usr_a, pw_a, _, err_a, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    let field_style = |idx: usize| {
        if app.login.field == idx { Style::default().fg(Color::Yellow) }
        else { Style::default() }
    };

    frame.render_widget(
        Paragraph::new(app.login.url.as_str())
            .block(Block::default().title(" Registry URL ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(field_style(0))),
        url_a,
    );
    frame.render_widget(
        Paragraph::new(app.login.username.as_str())
            .block(Block::default().title(" Username ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(field_style(1))),
        usr_a,
    );
    let pw_mask: String = "•".repeat(app.login.password.len());
    frame.render_widget(
        Paragraph::new(pw_mask.as_str())
            .block(Block::default().title(" Password ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(field_style(2))),
        pw_a,
    );
    if !app.login.error.is_empty() {
        frame.render_widget(
            Paragraph::new(app.login.error.as_str()).style(ERR_STYLE),
            err_a,
        );
    }
    frame.render_widget(
        Paragraph::new(" Tab/↑↓ move  Enter login  Esc quit").style(DIM_STYLE),
        hint_a,
    );
}

// ── Confirm dialog ────────────────────────────────────────────────────────────

fn draw_confirm(frame: &mut Frame, area: Rect, app: &App) {
    let Some(dlg) = &app.confirm else { return };
    let popup = center_rect(50, 7, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [msg_a, _, hint_a] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    frame.render_widget(
        Paragraph::new(dlg.message.as_str())
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center),
        msg_a,
    );
    frame.render_widget(
        Paragraph::new(" [y/Enter] confirm  [n/Esc] cancel")
            .style(DIM_STYLE)
            .alignment(Alignment::Center),
        hint_a,
    );
}

// ── Publish form ──────────────────────────────────────────────────────────────

fn draw_publish_form(frame: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.publish else { return };
    let popup = center_rect(54, 15, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Publish Package ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [na, va, pa, _, ha] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    let fs = |idx: usize| if form.field == idx { Style::default().fg(Color::Yellow) }
                          else { Style::default() };

    frame.render_widget(
        Paragraph::new(form.name.as_str())
            .block(Block::default().title(" Package name ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(fs(0))),
        na,
    );
    frame.render_widget(
        Paragraph::new(form.vers.as_str())
            .block(Block::default().title(" Version ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(fs(1))),
        va,
    );
    frame.render_widget(
        Paragraph::new(form.path.as_str())
            .block(Block::default().title(" Path to .tar.gz (Enter to publish) ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(fs(2))),
        pa,
    );
    frame.render_widget(
        Paragraph::new(" Tab/↑↓ move fields  Enter on path field to publish  Esc cancel")
            .style(DIM_STYLE),
        ha,
    );
}

// ── Create token form ─────────────────────────────────────────────────────────

fn draw_create_token_form(frame: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.tok_create else { return };
    let popup = center_rect(48, 10, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" New Token ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [name_a, scope_a, _, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    frame.render_widget(
        Paragraph::new(form.name.as_str())
            .block(Block::default().title(" Token name ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow))),
        name_a,
    );

    let scope_label = format!(
        " ◄ {} ► (Tab to cycle)",
        form.scope_str()
    );
    let scope_colour = match form.scope {
        0 => Color::Blue,
        2 => Color::Red,
        _ => Color::Green,
    };
    frame.render_widget(
        Paragraph::new(scope_label)
            .style(Style::default().fg(scope_colour))
            .block(Block::default().title(" Scope ").borders(Borders::ALL).border_type(BorderType::Rounded)),
        scope_a,
    );

    frame.render_widget(
        Paragraph::new(" Enter to create  Tab=cycle scope  Esc cancel").style(DIM_STYLE),
        hint_a,
    );
}

// ── Create org form ───────────────────────────────────────────────────────────

fn draw_create_org_form(frame: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.org_create else { return };
    let popup = center_rect(52, 12, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" New Organization ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [na, da, _, ha] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    let fs = |idx: usize| if form.field == idx { Style::default().fg(Color::Yellow) }
                          else { Style::default() };

    frame.render_widget(
        Paragraph::new(form.name.as_str())
            .block(Block::default().title(" Name ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(fs(0))),
        na,
    );
    frame.render_widget(
        Paragraph::new(form.description.as_str())
            .block(Block::default().title(" Description (optional) ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(fs(1))),
        da,
    );
    frame.render_widget(
        Paragraph::new(" Tab/↑↓ move  Enter to create  Esc cancel").style(DIM_STYLE),
        ha,
    );
}

// ── Add member form ───────────────────────────────────────────────────────────

fn draw_add_member_form(frame: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.add_member else { return };
    let popup = center_rect(48, 10, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" Add Member to '{}' ", form.org))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [usr_a, role_a, _, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    frame.render_widget(
        Paragraph::new(form.username.as_str())
            .block(Block::default().title(" Username ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow))),
        usr_a,
    );

    let role_label = format!(" ◄ {} ► (Tab to toggle)", form.role_str());
    let role_colour = if form.role == 1 { Color::Yellow } else { Color::Green };
    frame.render_widget(
        Paragraph::new(role_label)
            .style(Style::default().fg(role_colour))
            .block(Block::default().title(" Role ").borders(Borders::ALL).border_type(BorderType::Rounded)),
        role_a,
    );

    frame.render_widget(
        Paragraph::new(" Enter to add  Tab=toggle role  Esc cancel").style(DIM_STYLE),
        hint_a,
    );
}

// ── Add owner form ────────────────────────────────────────────────────────────

fn draw_add_owner_form(frame: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.add_owner else { return };
    let popup = center_rect(48, 7, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" Add Owner to '{}' ", form.pkg))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [usr_a, _, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    frame.render_widget(
        Paragraph::new(form.username.as_str())
            .block(Block::default().title(" Username ").borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow))),
        usr_a,
    );
    frame.render_widget(
        Paragraph::new(" Enter to add  Esc cancel").style(DIM_STYLE),
        hint_a,
    );
}

// ── New token reveal ──────────────────────────────────────────────────────────

fn draw_new_token(frame: &mut Frame, area: Rect, app: &App) {
    let Some(raw) = &app.new_tok_value else { return };
    let popup = center_rect(60, 9, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Token created — copy this now! ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [tok_a, _, hint_a] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ]).areas(inner);

    frame.render_widget(
        Paragraph::new(raw.as_str())
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: false }),
        tok_a,
    );
    frame.render_widget(
        Paragraph::new(" Press any key to continue").style(DIM_STYLE),
        hint_a,
    );
}

// ── Layout helpers ────────────────────────────────────────────────────────────

fn center_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}
