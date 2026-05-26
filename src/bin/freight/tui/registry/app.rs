use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::{ListState, TableState};
use tokio::sync::mpsc::Sender;

use super::client::{AuditEntry, Client, OrgMember, OrgSummary, PackageDetail, PackageSummary, TokenInfo, UserInfo};

// ── Tabs ──────────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum Tab {
    Packages = 0,
    Users    = 1,
    Tokens   = 2,
    Orgs     = 3,
    Audit    = 4,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Tab::Packages => Tab::Users,
            Tab::Users    => Tab::Tokens,
            Tab::Tokens   => Tab::Orgs,
            Tab::Orgs     => Tab::Audit,
            Tab::Audit    => Tab::Packages,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Tab::Packages => Tab::Audit,
            Tab::Users    => Tab::Packages,
            Tab::Tokens   => Tab::Users,
            Tab::Orgs     => Tab::Tokens,
            Tab::Audit    => Tab::Orgs,
        }
    }
    pub fn index(self) -> usize { self as usize }
}

// ── Modes / forms ─────────────────────────────────────────────────────────────

pub enum AppMode { Login, Main }

pub struct LoginState {
    pub url:      String,
    pub username: String,
    pub password: String,
    pub field:    usize,
    pub error:    String,
}

pub struct PublishForm {
    pub name:  String,
    pub vers:  String,
    pub path:  String,
    pub field: usize,
}

pub struct CreateTokenForm {
    pub name:  String,
    pub scope: usize, // 0=read, 1=publish, 2=admin
}

impl CreateTokenForm {
    pub fn scope_str(&self) -> &'static str {
        match self.scope { 0 => "read", 2 => "admin", _ => "publish" }
    }
}

pub struct CreateOrgForm {
    pub name:        String,
    pub description: String,
    pub field:       usize,
}

pub struct AddMemberForm {
    pub org:      String,
    pub username: String,
    pub role:     usize, // 0=member, 1=owner
}

impl AddMemberForm {
    pub fn role_str(&self) -> &'static str {
        if self.role == 1 { "owner" } else { "member" }
    }
}

pub struct AddOwnerForm {
    pub pkg:      String,
    pub username: String,
}

pub enum ConfirmAction {
    Yank        { name: String, version: String },
    Unyank      { name: String, version: String },
    DeletePkg   { name: String },
    RemoveUser  { username: String },
    RevokeToken { name: String },
    DeleteOrg   { name: String },
    RemoveMember { org: String, username: String },
    RemoveOwner  { pkg: String, username: String },
}

pub struct ConfirmDialog {
    pub message: String,
    pub action:  ConfirmAction,
}

pub enum DataEvent {
    Packages(Vec<PackageSummary>),
    PackageDetail(PackageDetail),
    Users(Vec<UserInfo>),
    Tokens(Vec<TokenInfo>),
    Orgs(Vec<OrgSummary>),
    OrgDetail(OrgSummary, Vec<OrgMember>),
    Audit(Vec<AuditEntry>),
    Me { username: String, is_admin: bool },
    LoginSuccess { url: String, token: String, username: String, is_admin: bool },
    NewToken(String),
    Done(String),
    Err(String),
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub client:  Client,
    pub mode:    AppMode,
    pub tab:     Tab,
    pub status:  String,
    pub is_err:  bool,
    pub loading: bool,

    pub login:        LoginState,
    pub current_user: String,
    pub is_admin:     bool,

    // Packages tab
    pub packages:      Vec<PackageSummary>,
    pub pkg_state:     ListState,
    pub pkg_search:    String,
    pub pkg_search_on: bool,
    pub pkg_detail:    Option<PackageDetail>,
    pub ver_state:     ListState,
    pub publish:       Option<PublishForm>,
    pub add_owner:     Option<AddOwnerForm>,

    // Users tab
    pub users:     Vec<UserInfo>,
    pub usr_state: TableState,

    // Tokens tab
    pub tokens:        Vec<TokenInfo>,
    pub tok_state:     TableState,
    pub tok_create:    Option<CreateTokenForm>,
    pub new_tok_value: Option<String>,

    // Orgs tab
    pub orgs:        Vec<OrgSummary>,
    pub org_state:   ListState,
    pub org_detail:  Option<OrgSummary>,
    pub org_members: Vec<OrgMember>,
    pub org_mem_state: TableState,
    pub org_create:  Option<CreateOrgForm>,
    pub add_member:  Option<AddMemberForm>,

    // Audit tab
    pub audit:         Vec<AuditEntry>,
    pub aud_state:     TableState,
    pub aud_filter:    String,
    pub aud_filter_on: bool,

    // Confirm dialog
    pub confirm: Option<ConfirmDialog>,
}

impl App {
    pub fn new(client: Client, base_url: String) -> Self {
        let has_token = client.has_token();
        Self {
            client,
            mode:    if has_token { AppMode::Main } else { AppMode::Login },
            tab:     Tab::Packages,
            status:  String::new(),
            is_err:  false,
            loading: false,
            login: LoginState {
                url: base_url, username: String::new(),
                password: String::new(), field: 1, error: String::new(),
            },
            current_user:  String::new(),
            is_admin:      false,
            packages:      Vec::new(),
            pkg_state:     ListState::default(),
            pkg_search:    String::new(),
            pkg_search_on: false,
            pkg_detail:    None,
            ver_state:     ListState::default(),
            publish:       None,
            add_owner:     None,
            users:         Vec::new(),
            usr_state:     TableState::default(),
            tokens:        Vec::new(),
            tok_state:     TableState::default(),
            tok_create:    None,
            new_tok_value: None,
            orgs:          Vec::new(),
            org_state:     ListState::default(),
            org_detail:    None,
            org_members:   Vec::new(),
            org_mem_state: TableState::default(),
            org_create:    None,
            add_member:    None,
            audit:         Vec::new(),
            aud_state:     TableState::default(),
            aud_filter:    String::new(),
            aud_filter_on: false,
            confirm:       None,
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>, err: bool) {
        self.status = msg.into();
        self.is_err = err;
    }

    // ── Data event handler ────────────────────────────────────────────────────

    pub fn handle_data(&mut self, ev: DataEvent, tx: &Sender<DataEvent>) {
        self.loading = false;
        match ev {
            DataEvent::Packages(pkgs) => {
                self.packages = pkgs;
                if self.pkg_state.selected().is_none() && !self.packages.is_empty() {
                    self.pkg_state.select(Some(0));
                }
            }
            DataEvent::PackageDetail(d) => {
                self.ver_state.select(Some(0));
                self.pkg_detail = Some(d);
            }
            DataEvent::Users(u) => {
                self.users = u;
                if !self.users.is_empty() { self.usr_state.select(Some(0)); }
            }
            DataEvent::Tokens(t) => {
                self.tokens = t;
                if !self.tokens.is_empty() { self.tok_state.select(Some(0)); }
            }
            DataEvent::Orgs(o) => {
                self.orgs = o;
                if self.org_state.selected().is_none() && !self.orgs.is_empty() {
                    self.org_state.select(Some(0));
                }
            }
            DataEvent::OrgDetail(org, members) => {
                self.org_detail = Some(org);
                self.org_members = members;
                if !self.org_members.is_empty() { self.org_mem_state.select(Some(0)); }
            }
            DataEvent::Audit(a) => {
                self.audit = a;
                if !self.audit.is_empty() { self.aud_state.select(Some(0)); }
            }
            DataEvent::Me { username, is_admin } => {
                self.current_user = username;
                self.is_admin = is_admin;
            }
            DataEvent::LoginSuccess { url, token, username, is_admin } => {
                self.client = Client::new(url, Some(token));
                self.current_user = username;
                self.is_admin = is_admin;
                self.mode = AppMode::Main;
                self.load_current_tab(tx.clone());
                self.set_status("Logged in", false);
            }
            DataEvent::NewToken(raw) => {
                self.new_tok_value = Some(raw);
                self.tok_create = None;
                self.set_status("Token created — copy it now, it won't be shown again", false);
            }
            DataEvent::Done(msg) => {
                self.set_status(msg, false);
            }
            DataEvent::Err(e) => {
                self.set_status(e, true);
            }
        }
    }

    // ── Data loaders ──────────────────────────────────────────────────────────

    pub fn load_packages(&mut self, tx: Sender<DataEvent>) {
        let client = self.client.clone();
        let q = self.pkg_search.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.search(&q).await {
                Ok(p)  => { tx.send(DataEvent::Packages(p)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_package_detail(&mut self, name: String, tx: Sender<DataEvent>) {
        let client = self.client.clone();
        self.loading = true;
        self.pkg_detail = None;
        tokio::spawn(async move {
            match client.get_package(&name).await {
                Ok(d)  => { tx.send(DataEvent::PackageDetail(d)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_users(&mut self, tx: Option<Sender<DataEvent>>) {
        let Some(tx) = tx else { return };
        let client = self.client.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.list_users().await {
                Ok(u)  => { tx.send(DataEvent::Users(u)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_tokens(&mut self, tx: Option<Sender<DataEvent>>) {
        let Some(tx) = tx else { return };
        let client = self.client.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.list_tokens().await {
                Ok(t)  => { tx.send(DataEvent::Tokens(t)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_orgs(&mut self, tx: Sender<DataEvent>) {
        let client = self.client.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.list_orgs().await {
                Ok(o)  => { tx.send(DataEvent::Orgs(o)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_org_detail(&mut self, name: String, tx: Sender<DataEvent>) {
        let client = self.client.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.get_org(&name).await {
                Ok((org, members)) => { tx.send(DataEvent::OrgDetail(org, members)).await.ok(); }
                Err(e)             => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_audit(&mut self, tx: Option<Sender<DataEvent>>) {
        let Some(tx) = tx else { return };
        let client = self.client.clone();
        let filter = self.aud_filter.clone();
        self.loading = true;
        tokio::spawn(async move {
            match client.list_audit(&filter).await {
                Ok(a)  => { tx.send(DataEvent::Audit(a)).await.ok(); }
                Err(e) => { tx.send(DataEvent::Err(e.to_string())).await.ok(); }
            }
        });
    }

    pub fn load_me(&mut self, tx: Sender<DataEvent>) {
        let client = self.client.clone();
        tokio::spawn(async move {
            match client.me().await {
                Ok((u, a)) => { tx.send(DataEvent::Me { username: u, is_admin: a }).await.ok(); }
                Err(_)     => {}
            }
        });
    }

    pub fn load_current_tab(&mut self, tx: Sender<DataEvent>) {
        match self.tab {
            Tab::Packages => self.load_packages(tx),
            Tab::Users    => self.load_users(Some(tx)),
            Tab::Tokens   => self.load_tokens(Some(tx)),
            Tab::Orgs     => self.load_orgs(tx),
            Tab::Audit    => self.load_audit(Some(tx)),
        }
    }

    // ── Key handler ───────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }

        if matches!(self.mode, AppMode::Login) {
            return self.handle_login(key, tx);
        }

        if self.confirm.is_some() {
            return self.handle_confirm(key, tx);
        }

        if self.new_tok_value.is_some() {
            self.new_tok_value = None;
            return false;
        }

        if self.publish.is_some()     { return self.handle_publish(key, tx); }
        if self.tok_create.is_some()  { return self.handle_create_token(key, tx); }
        if self.org_create.is_some()  { return self.handle_create_org(key, tx); }
        if self.add_member.is_some()  { return self.handle_add_member(key, tx); }
        if self.add_owner.is_some()   { return self.handle_add_owner(key, tx); }
        if self.pkg_search_on         { return self.handle_pkg_search_input(key, tx); }
        if self.aud_filter_on         { return self.handle_aud_filter_input(key, tx); }

        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return true,
            KeyCode::Tab      => { self.switch_tab(self.tab.next(), tx); }
            KeyCode::BackTab  => { self.switch_tab(self.tab.prev(), tx); }
            KeyCode::Char('1') => self.switch_tab(Tab::Packages, tx),
            KeyCode::Char('2') => self.switch_tab(Tab::Users, tx),
            KeyCode::Char('3') => self.switch_tab(Tab::Tokens, tx),
            KeyCode::Char('4') => self.switch_tab(Tab::Orgs, tx),
            KeyCode::Char('5') => self.switch_tab(Tab::Audit, tx),
            _ => {}
        }

        match self.tab {
            Tab::Packages => self.handle_packages(key, tx),
            Tab::Users    => self.handle_users(key, tx),
            Tab::Tokens   => self.handle_tokens(key, tx),
            Tab::Orgs     => self.handle_orgs(key, tx),
            Tab::Audit    => self.handle_audit_key(key, tx),
        }

        false
    }

    fn switch_tab(&mut self, tab: Tab, tx: &Sender<DataEvent>) {
        self.tab = tab;
        self.pkg_detail = None;
        self.org_detail = None;
        self.load_current_tab(tx.clone());
    }

    // ── Login ─────────────────────────────────────────────────────────────────

    fn handle_login(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.login.field = (self.login.field + 1) % 3;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.login.field = (self.login.field + 2) % 3;
            }
            KeyCode::Char(c) => {
                match self.login.field {
                    0 => self.login.url.push(c),
                    1 => self.login.username.push(c),
                    _ => self.login.password.push(c),
                }
            }
            KeyCode::Backspace => {
                match self.login.field {
                    0 => { self.login.url.pop(); }
                    1 => { self.login.username.pop(); }
                    _ => { self.login.password.pop(); }
                }
            }
            KeyCode::Enter => {
                let username = self.login.username.clone();
                let password = self.login.password.clone();
                let url      = self.login.url.clone();
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    let client = Client::new(url.clone(), None);
                    match client.login(&username, &password).await {
                        Ok(resp) => {
                            let token = resp.token;
                            let authed = Client::new(url.clone(), Some(token.clone()));
                            let (uname, is_admin) = authed.me().await.unwrap_or_default();
                            super::config::TuiConfig { url: url.clone(), token: token.clone() }.save();
                            tx2.send(DataEvent::LoginSuccess {
                                url, token, username: uname, is_admin,
                            }).await.ok();
                        }
                        Err(e) => {
                            tx2.send(DataEvent::Err(e.to_string())).await.ok();
                        }
                    }
                });
                self.loading = true;
            }
            KeyCode::Esc => return true,
            _ => {}
        }
        false
    }

    // ── Confirm ───────────────────────────────────────────────────────────────

    fn handle_confirm(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(dlg) = self.confirm.take() {
                    self.execute_action(dlg.action, tx);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.confirm = None;
            }
            _ => {}
        }
        false
    }

    fn execute_action(&mut self, action: ConfirmAction, tx: &Sender<DataEvent>) {
        let client = self.client.clone();
        let tx2    = tx.clone();
        match action {
            ConfirmAction::Yank { name, version } => {
                tokio::spawn(async move {
                    match client.yank(&name, &version).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("yanked {name}@{version}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.reload_detail(tx);
            }
            ConfirmAction::Unyank { name, version } => {
                tokio::spawn(async move {
                    match client.unyank(&name, &version).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("unyanked {name}@{version}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.reload_detail(tx);
            }
            ConfirmAction::DeletePkg { name } => {
                let tx3 = tx.clone();
                tokio::spawn(async move {
                    match client.delete_package(&name).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("deleted {name}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.pkg_detail = None;
                self.load_packages(tx3);
            }
            ConfirmAction::RemoveUser { username } => {
                let tx3 = tx.clone();
                tokio::spawn(async move {
                    match client.remove_user(&username).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("removed {username}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_users(Some(tx3));
            }
            ConfirmAction::RevokeToken { name } => {
                let tx3 = tx.clone();
                tokio::spawn(async move {
                    match client.revoke_token(&name).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("revoked {name}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_tokens(Some(tx3));
            }
            ConfirmAction::DeleteOrg { name } => {
                let tx3 = tx.clone();
                tokio::spawn(async move {
                    match client.delete_org(&name).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("deleted org {name}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.org_detail = None;
                self.load_orgs(tx3);
            }
            ConfirmAction::RemoveMember { org, username } => {
                let org2 = org.clone();
                let tx3  = tx.clone();
                tokio::spawn(async move {
                    match client.remove_org_member(&org, &username).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("removed {username} from {org}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_org_detail(org2, tx3);
            }
            ConfirmAction::RemoveOwner { pkg, username } => {
                let pkg2 = pkg.clone();
                let tx3  = tx.clone();
                tokio::spawn(async move {
                    match client.remove_owner(&pkg, &username).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("removed owner {username} from {pkg}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_package_detail(pkg2, tx3);
            }
        }
    }

    fn reload_detail(&mut self, tx: &Sender<DataEvent>) {
        if let Some(name) = self.pkg_detail.as_ref().map(|d| d.name.clone()) {
            self.load_package_detail(name, tx.clone());
        }
    }

    // ── Packages tab ──────────────────────────────────────────────────────────

    fn handle_pkg_search_input(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.pkg_search_on = false;
                self.load_packages(tx.clone());
            }
            KeyCode::Char(c) => self.pkg_search.push(c),
            KeyCode::Backspace => { self.pkg_search.pop(); }
            _ => {}
        }
        false
    }

    fn handle_packages(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) {
        match key.code {
            KeyCode::Char('/') => { self.pkg_search_on = true; }
            KeyCode::Char('r') | KeyCode::F(5) => self.load_packages(tx.clone()),
            KeyCode::Char('P') => {
                self.publish = Some(PublishForm {
                    name: String::new(), vers: String::new(), path: String::new(), field: 0,
                });
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.pkg_detail.is_some() {
                    list_next(&mut self.ver_state,
                        self.pkg_detail.as_ref().map(|d| d.versions.len()).unwrap_or(0));
                } else {
                    list_next(&mut self.pkg_state, self.packages.len());
                    self.load_selected_package(tx);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.pkg_detail.is_some() {
                    list_prev(&mut self.ver_state);
                } else {
                    list_prev(&mut self.pkg_state);
                    self.load_selected_package(tx);
                }
            }
            KeyCode::Enter => {
                if self.pkg_detail.is_none() {
                    self.load_selected_package(tx);
                }
            }
            KeyCode::Esc => { self.pkg_detail = None; }
            KeyCode::Char('y') => {
                if let Some((name, version)) = self.selected_version() {
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Yank {name}@{version}?"),
                        action: ConfirmAction::Yank { name, version },
                    });
                }
            }
            KeyCode::Char('u') => {
                if let Some((name, version)) = self.selected_version() {
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Unyank {name}@{version}?"),
                        action: ConfirmAction::Unyank { name, version },
                    });
                }
            }
            // Add owner
            KeyCode::Char('a') => {
                if let Some(detail) = &self.pkg_detail {
                    self.add_owner = Some(AddOwnerForm {
                        pkg: detail.name.clone(), username: String::new(),
                    });
                }
            }
            // Remove selected owner (cycle through owners list with O)
            KeyCode::Char('O') => {
                if let Some(detail) = &self.pkg_detail {
                    if let Some(owner) = detail.owners.first() {
                        let pkg = detail.name.clone();
                        let owner = owner.clone();
                        self.confirm = Some(ConfirmDialog {
                            message: format!("Remove owner '{owner}' from {pkg}?"),
                            action: ConfirmAction::RemoveOwner { pkg, username: owner },
                        });
                    }
                }
            }
            KeyCode::Char('d') if self.is_admin => {
                if let Some(d) = &self.pkg_detail {
                    let name = d.name.clone();
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Hard-delete package {name} and ALL versions?"),
                        action: ConfirmAction::DeletePkg { name },
                    });
                }
            }
            _ => {}
        }
    }

    fn load_selected_package(&mut self, tx: &Sender<DataEvent>) {
        if let Some(idx) = self.pkg_state.selected() {
            if let Some(pkg) = self.packages.get(idx) {
                self.load_package_detail(pkg.name.clone(), tx.clone());
            }
        }
    }

    fn selected_version(&self) -> Option<(String, String)> {
        let detail = self.pkg_detail.as_ref()?;
        let idx    = self.ver_state.selected()?;
        let ver    = detail.versions.get(idx)?;
        Some((detail.name.clone(), ver.version.clone()))
    }

    // ── Publish form ──────────────────────────────────────────────────────────

    fn handle_publish(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        let form = self.publish.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => { self.publish = None; }
            KeyCode::Tab | KeyCode::Down => { form.field = (form.field + 1) % 3; }
            KeyCode::BackTab | KeyCode::Up => { form.field = (form.field + 2) % 3; }
            KeyCode::Char(c) => match form.field {
                0 => form.name.push(c),
                1 => form.vers.push(c),
                _ => form.path.push(c),
            },
            KeyCode::Backspace => match form.field {
                0 => { form.name.pop(); }
                1 => { form.vers.pop(); }
                _ => { form.path.pop(); }
            },
            KeyCode::Enter if form.field == 2 => {
                let name = form.name.clone();
                let vers = form.vers.clone();
                let path = form.path.clone();
                self.publish = None;
                let client = self.client.clone();
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    let tarball = match tokio::fs::read(&path).await {
                        Ok(b) => b,
                        Err(e) => {
                            tx2.send(DataEvent::Err(format!("cannot read {path}: {e}"))).await.ok();
                            return;
                        }
                    };
                    match client.publish(&name, &vers, tarball).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("published {name}@{vers}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
            }
            _ => {}
        }
        false
    }

    // ── Add owner form ────────────────────────────────────────────────────────

    fn handle_add_owner(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        let form = self.add_owner.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => { self.add_owner = None; }
            KeyCode::Char(c) => form.username.push(c),
            KeyCode::Backspace => { form.username.pop(); }
            KeyCode::Enter => {
                let pkg      = form.pkg.clone();
                let username = form.username.clone();
                self.add_owner = None;
                let client = self.client.clone();
                let tx2    = tx.clone();
                let tx3    = tx.clone();
                let pkg2   = pkg.clone();
                tokio::spawn(async move {
                    match client.add_owner(&pkg, &username).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("added {username} as owner of {pkg}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_package_detail(pkg2, tx3);
            }
            _ => {}
        }
        false
    }

    // ── Users tab ─────────────────────────────────────────────────────────────

    fn handle_users(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) {
        match key.code {
            KeyCode::Char('r') | KeyCode::F(5) => self.load_users(Some(tx.clone())),
            KeyCode::Down | KeyCode::Char('j') => list_next(&mut self.usr_state, self.users.len()),
            KeyCode::Up   | KeyCode::Char('k') => list_prev(&mut self.usr_state),
            KeyCode::Char('p') if self.is_admin => {
                if let Some(u) = self.selected_user() {
                    let tx2 = tx.clone();
                    let client = self.client.clone();
                    let tx3 = tx.clone();
                    tokio::spawn(async move {
                        match client.promote_user(&u).await {
                            Ok(_)  => { tx2.send(DataEvent::Done(format!("promoted {u}"))).await.ok(); }
                            Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                        }
                    });
                    self.load_users(Some(tx3));
                }
            }
            KeyCode::Char('d') if self.is_admin => {
                if let Some(u) = self.selected_user() {
                    let tx2 = tx.clone();
                    let client = self.client.clone();
                    let tx3 = tx.clone();
                    tokio::spawn(async move {
                        match client.demote_user(&u).await {
                            Ok(_)  => { tx2.send(DataEvent::Done(format!("demoted {u}"))).await.ok(); }
                            Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                        }
                    });
                    self.load_users(Some(tx3));
                }
            }
            KeyCode::Char('x') if self.is_admin => {
                if let Some(u) = self.selected_user() {
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Remove user {u}?"),
                        action: ConfirmAction::RemoveUser { username: u },
                    });
                }
            }
            _ => {}
        }
    }

    fn selected_user(&self) -> Option<String> {
        let idx = self.usr_state.selected()?;
        self.users.get(idx).map(|u| u.username.clone())
    }

    // ── Tokens tab ────────────────────────────────────────────────────────────

    fn handle_create_token(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        let form = self.tok_create.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => { self.tok_create = None; }
            KeyCode::Tab => { form.scope = (form.scope + 1) % 3; }
            KeyCode::BackTab => { form.scope = (form.scope + 2) % 3; }
            KeyCode::Char(c) => form.name.push(c),
            KeyCode::Backspace => { form.name.pop(); }
            KeyCode::Enter => {
                let name  = form.name.clone();
                let scope = form.scope_str().to_string();
                self.tok_create = None;
                let client = self.client.clone();
                let tx2    = tx.clone();
                let tx3    = tx.clone();
                tokio::spawn(async move {
                    match client.create_token(&name, None, &scope).await {
                        Ok(raw) => { tx2.send(DataEvent::NewToken(raw)).await.ok(); }
                        Err(e)  => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_tokens(Some(tx3));
            }
            _ => {}
        }
        false
    }

    fn handle_tokens(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) {
        match key.code {
            KeyCode::Char('r') | KeyCode::F(5) => self.load_tokens(Some(tx.clone())),
            KeyCode::Down | KeyCode::Char('j') => list_next(&mut self.tok_state, self.tokens.len()),
            KeyCode::Up   | KeyCode::Char('k') => list_prev(&mut self.tok_state),
            KeyCode::Char('n') => {
                self.tok_create = Some(CreateTokenForm { name: String::new(), scope: 1 });
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(name) = self.selected_token() {
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Revoke token '{name}'?"),
                        action: ConfirmAction::RevokeToken { name },
                    });
                }
            }
            _ => {}
        }
    }

    fn selected_token(&self) -> Option<String> {
        let idx = self.tok_state.selected()?;
        self.tokens.get(idx).map(|t| t.name.clone())
    }

    // ── Orgs tab ──────────────────────────────────────────────────────────────

    fn handle_orgs(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) {
        match key.code {
            KeyCode::Char('r') | KeyCode::F(5) => {
                self.org_detail = None;
                self.load_orgs(tx.clone());
            }
            KeyCode::Char('n') => {
                self.org_create = Some(CreateOrgForm {
                    name: String::new(), description: String::new(), field: 0,
                });
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.org_detail.is_some() {
                    list_next(&mut self.org_mem_state, self.org_members.len());
                } else {
                    list_next(&mut self.org_state, self.orgs.len());
                    self.load_selected_org(tx);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.org_detail.is_some() {
                    list_prev(&mut self.org_mem_state);
                } else {
                    list_prev(&mut self.org_state);
                    self.load_selected_org(tx);
                }
            }
            KeyCode::Enter => {
                if self.org_detail.is_none() {
                    self.load_selected_org(tx);
                }
            }
            KeyCode::Esc => { self.org_detail = None; }
            KeyCode::Char('a') => {
                if let Some(org) = &self.org_detail {
                    self.add_member = Some(AddMemberForm {
                        org: org.name.clone(), username: String::new(), role: 0,
                    });
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(org) = &self.org_detail {
                    if let Some(idx) = self.org_mem_state.selected() {
                        if let Some(m) = self.org_members.get(idx) {
                            let org_name = org.name.clone();
                            let uname    = m.username.clone();
                            self.confirm = Some(ConfirmDialog {
                                message: format!("Remove {uname} from {org_name}?"),
                                action: ConfirmAction::RemoveMember { org: org_name, username: uname },
                            });
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(org) = &self.org_detail {
                    let name = org.name.clone();
                    self.confirm = Some(ConfirmDialog {
                        message: format!("Delete org '{name}'?"),
                        action: ConfirmAction::DeleteOrg { name },
                    });
                } else if let Some(idx) = self.org_state.selected() {
                    if let Some(org) = self.orgs.get(idx) {
                        let name = org.name.clone();
                        self.confirm = Some(ConfirmDialog {
                            message: format!("Delete org '{name}'?"),
                            action: ConfirmAction::DeleteOrg { name },
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn load_selected_org(&mut self, tx: &Sender<DataEvent>) {
        if let Some(idx) = self.org_state.selected() {
            if let Some(org) = self.orgs.get(idx) {
                self.load_org_detail(org.name.clone(), tx.clone());
            }
        }
    }

    fn handle_create_org(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        let form = self.org_create.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => { self.org_create = None; }
            KeyCode::Tab | KeyCode::Down => { form.field = (form.field + 1) % 2; }
            KeyCode::BackTab | KeyCode::Up => { form.field = (form.field + 1) % 2; }
            KeyCode::Char(c) => match form.field {
                0 => form.name.push(c),
                _ => form.description.push(c),
            },
            KeyCode::Backspace => match form.field {
                0 => { form.name.pop(); }
                _ => { form.description.pop(); }
            },
            KeyCode::Enter => {
                let name = form.name.clone();
                let desc = form.description.clone();
                self.org_create = None;
                let client = self.client.clone();
                let tx2    = tx.clone();
                let tx3    = tx.clone();
                tokio::spawn(async move {
                    match client.create_org(&name, &desc).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("created org {name}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_orgs(tx3);
            }
            _ => {}
        }
        false
    }

    fn handle_add_member(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        let form = self.add_member.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => { self.add_member = None; }
            KeyCode::Tab => { form.role = (form.role + 1) % 2; }
            KeyCode::Char(c) => form.username.push(c),
            KeyCode::Backspace => { form.username.pop(); }
            KeyCode::Enter => {
                let org      = form.org.clone();
                let username = form.username.clone();
                let role     = form.role_str().to_string();
                self.add_member = None;
                let client   = self.client.clone();
                let tx2      = tx.clone();
                let org2     = org.clone();
                let tx3      = tx.clone();
                tokio::spawn(async move {
                    match client.add_org_member(&org, &username, &role).await {
                        Ok(_)  => { tx2.send(DataEvent::Done(format!("added {username} to {org} as {role}"))).await.ok(); }
                        Err(e) => { tx2.send(DataEvent::Err(e.to_string())).await.ok(); }
                    }
                });
                self.load_org_detail(org2, tx3);
            }
            _ => {}
        }
        false
    }

    // ── Audit tab ─────────────────────────────────────────────────────────────

    fn handle_aud_filter_input(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) -> bool {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.aud_filter_on = false;
                self.load_audit(Some(tx.clone()));
            }
            KeyCode::Char(c) => self.aud_filter.push(c),
            KeyCode::Backspace => { self.aud_filter.pop(); }
            _ => {}
        }
        false
    }

    fn handle_audit_key(&mut self, key: KeyEvent, tx: &Sender<DataEvent>) {
        match key.code {
            KeyCode::Char('/') => { self.aud_filter_on = true; }
            KeyCode::Char('r') | KeyCode::F(5) => self.load_audit(Some(tx.clone())),
            KeyCode::Down | KeyCode::Char('j') => list_next(&mut self.aud_state, self.audit.len()),
            KeyCode::Up   | KeyCode::Char('k') => list_prev(&mut self.aud_state),
            _ => {}
        }
    }
}

// ── List/Table navigation helpers ─────────────────────────────────────────────

trait SelectState {
    fn selected(&self) -> Option<usize>;
    fn select(&mut self, idx: Option<usize>);
}

impl SelectState for ListState {
    fn selected(&self) -> Option<usize> { ListState::selected(self) }
    fn select(&mut self, idx: Option<usize>) { ListState::select(self, idx); }
}

impl SelectState for TableState {
    fn selected(&self) -> Option<usize> { TableState::selected(self) }
    fn select(&mut self, idx: Option<usize>) { TableState::select(self, idx); }
}

fn list_next(state: &mut impl SelectState, len: usize) {
    if len == 0 { return; }
    let i = state.selected().map(|i| (i + 1).min(len - 1)).unwrap_or(0);
    state.select(Some(i));
}

fn list_prev(state: &mut impl SelectState) {
    let i = state.selected().map(|i| i.saturating_sub(1)).unwrap_or(0);
    state.select(Some(i));
}
