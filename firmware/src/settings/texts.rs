//! NVS storage for templates, automations, and live status message policies.

use std::sync::atomic::Ordering;

use autotext::automation::{Automation, MAX_AUTOMATIONS};
use autotext::library::Library;
use autotext::status::{Live, StatusConfig};
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use power_core::texts::error::TextsError;
use power_core::texts::{self, Templates};

use super::{read, Settings};

const NAMES_KEY: &str = "#names";
const N_KEY: &str = "n";
const STATUS_CFG: &str = "status_cfg";
const STATUS_LIVE: &str = "status_live";

struct TplNvsLib<'a>(&'a EspNvs<NvsDefault>);

impl Library for TplNvsLib<'_> {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        let Ok(Some(len)) = self.0.str_len(name) else { return false; };
        let mut buf = vec![0u8; len];
        let Ok(Some(stored)) = self.0.get_str(name, &mut buf) else { return false; };
        with(stored);
        true
    }
}

impl Settings {
    /// Loads or initializes templates, automations, and status configuration.
    pub(super) fn load_texts(
        tpl: &EspNvs<NvsDefault>,
        auto: &EspNvs<NvsDefault>,
        nvs: &EspNvs<NvsDefault>,
    ) -> anyhow::Result<(Templates, Vec<Automation>, StatusConfig)> {
        let names_stored = read(tpl, NAMES_KEY, |s| Ok::<_, ()>(s.to_string()));
        match names_stored {
            None => {
                // First boot or update from old firmware: install defaults
                let mut tmpls = Templates::new();
                for (name, text) in texts::default_templates() {
                    tpl.set_str(name, text)?;
                    let _ = tmpls.add(name);
                }
                tpl.set_str(NAMES_KEY, &tmpls.to_stored())?;

                let default_autos = texts::default_automations();
                auto.set_u32(N_KEY, default_autos.len() as u32)?;
                for (i, a) in default_autos.iter().enumerate() {
                    let form = texts::automation_to_form(a);
                    auto.set_str(&format!("a{i}"), &form)?;
                }

                let default_cfg = texts::default_status_config();
                let form = texts::status_config_to_form(&default_cfg);
                nvs.set_str(STATUS_CFG, &form)?;

                Ok((tmpls, default_autos, default_cfg))
            }
            Some(stored) => {
                let tmpls = Templates::from_stored(&stored);
                let n = auto.get_u32(N_KEY).ok().flatten().unwrap_or(0);
                let mut autos = Vec::new();
                let lib = TplNvsLib(tpl);
                for i in 0..n {
                    if let Some(form) = read(auto, &format!("a{i}"), |s| Ok::<_, ()>(s.to_string())) {
                        if let Ok(a) = texts::automation_from_form(&form, &lib, &|name| tmpls.contains(name)) {
                            autos.push(a);
                        }
                    }
                }

                let status_config = read(nvs, STATUS_CFG, texts::status_config_from_form)
                    .unwrap_or_else(texts::default_status_config);

                Ok((tmpls, autos, status_config))
            }
        }
    }

    /// List of registered template names.
    pub fn templates(&self) -> Vec<String> {
        self.templates.read().unwrap().names().to_vec()
    }

    /// Reads a template's text by name from NVS, if present.
    pub fn template(&self, name: &str) -> Option<String> {
        let tpl = self.tpl.lock().unwrap();
        let len = tpl.str_len(name).ok().flatten()?;
        let mut buf = vec![0u8; len];
        tpl.get_str(name, &mut buf).ok().flatten().map(|s| s.to_string())
    }

    /// Checks whether a template name exists.
    pub fn has_template(&self, name: &str) -> bool {
        self.templates.read().unwrap().contains(name)
    }

    /// Validates and saves a template to NVS.
    pub fn save_template(&self, name: &str, text: &str) -> anyhow::Result<Result<(), TextsError>> {
        if let Err(e) = texts::check_template(name, text, self) {
            return Ok(Err(e));
        }
        let mut tmpls = self.templates.write().unwrap();
        if !tmpls.contains(name) {
            if let Err(e) = tmpls.add(name) {
                return Ok(Err(e));
            }
            let tpl = self.tpl.lock().unwrap();
            tpl.set_str(name, text)?;
            tpl.set_str(NAMES_KEY, &tmpls.to_stored())?;
        } else {
            let tpl = self.tpl.lock().unwrap();
            tpl.set_str(name, text)?;
        }
        Ok(Ok(()))
    }

    /// Deletes a template from NVS after checking that no other template or automation calls it.
    pub fn delete_template(&self, name: &str) -> anyhow::Result<Result<(), TextsError>> {
        let tmpls = self.templates.read().unwrap();
        if !tmpls.contains(name) {
            return Ok(Err(TextsError::TemplateNotFound(name.to_string())));
        }
        // Read each template text once to check callers
        let mut loaded: Vec<(String, String)> = Vec::with_capacity(tmpls.names().len());
        for n in tmpls.names() {
            if let Some(text) = self.template(n) {
                loaded.push((n.clone(), text));
            }
        }
        let tpl_refs: Vec<(&str, &str)> = loaded.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
        let autos = self.automations.read().unwrap();
        if let Err(e) = texts::check_delete_template(name, tpl_refs, &*autos) {
            return Ok(Err(e));
        }
        drop(autos);
        drop(tmpls);

        let mut tmpls = self.templates.write().unwrap();
        tmpls.remove(name);
        let tpl = self.tpl.lock().unwrap();
        let _ = tpl.remove(name);
        tpl.set_str(NAMES_KEY, &tmpls.to_stored())?;
        Ok(Ok(()))
    }

    /// Returns the current automations list.
    pub fn automations(&self) -> Vec<Automation> {
        self.automations.read().unwrap().clone()
    }

    /// Returns the current automations and generation counter.
    pub fn automations_with_gen(&self) -> (Vec<Automation>, u32) {
        (self.automations.read().unwrap().clone(), self.auto_generation.load(Ordering::Relaxed))
    }

    /// Parses and saves an automation (updating an existing index or adding a new rule).
    pub fn save_automation(&self, index: Option<usize>, form: &str) -> anyhow::Result<Result<(), TextsError>> {
        let auto = match texts::automation_from_form(form, self, &|n| self.has_template(n)) {
            Ok(a) => a,
            Err(e) => return Ok(Err(e)),
        };
        let mut autos = self.automations.write().unwrap();
        let old_count = autos.len();
        match index {
            Some(i) => {
                if i >= autos.len() {
                    return Ok(Err(TextsError::MissingField("index")));
                }
                autos[i] = auto;
            }
            None => {
                if autos.len() >= MAX_AUTOMATIONS {
                    return Ok(Err(TextsError::TooManyTemplates));
                }
                autos.push(auto);
            }
        }
        let nvs = self.auto.lock().unwrap();
        nvs.set_u32(N_KEY, autos.len() as u32)?;
        for (i, a) in autos.iter().enumerate() {
            let form_str = texts::automation_to_form(a);
            nvs.set_str(&format!("a{i}"), &form_str)?;
        }
        if old_count > autos.len() {
            for i in autos.len()..old_count {
                let _ = nvs.remove(&format!("a{i}"));
            }
        }
        self.auto_generation.fetch_add(1, Ordering::Relaxed);
        Ok(Ok(()))
    }

    /// Deletes an automation by index.
    pub fn delete_automation(&self, index: usize) -> anyhow::Result<Result<(), TextsError>> {
        let mut autos = self.automations.write().unwrap();
        if index >= autos.len() {
            return Ok(Err(TextsError::MissingField("index")));
        }
        let old_count = autos.len();
        autos.remove(index);
        let nvs = self.auto.lock().unwrap();
        nvs.set_u32(N_KEY, autos.len() as u32)?;
        for (i, a) in autos.iter().enumerate() {
            let form_str = texts::automation_to_form(a);
            nvs.set_str(&format!("a{i}"), &form_str)?;
        }
        for i in autos.len()..old_count {
            let _ = nvs.remove(&format!("a{i}"));
        }
        self.auto_generation.fetch_add(1, Ordering::Relaxed);
        Ok(Ok(()))
    }

    /// Current live status configuration.
    pub fn status_config(&self) -> StatusConfig {
        self.status_config.read().unwrap().clone()
    }

    /// Parses and saves the live status configuration.
    pub fn save_status_config(&self, form: &str) -> anyhow::Result<Result<(), TextsError>> {
        let cfg = match texts::status_config_from_form(form) {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        let form_str = texts::status_config_to_form(&cfg);
        self.write(STATUS_CFG, &form_str)?;
        *self.status_config.write().unwrap() = cfg;
        Ok(Ok(()))
    }

    /// Last saved live status message state, if any.
    pub fn live(&self) -> Option<Live> {
        read(&self.nvs.lock().unwrap(), STATUS_LIVE, texts::live_from_form)
    }

    /// Saves or clears the live status message state in NVS.
    pub fn save_live(&self, live: Option<&Live>) -> anyhow::Result<()> {
        let nvs = self.nvs.lock().unwrap();
        match live {
            Some(l) => {
                let form = texts::live_to_form(l);
                nvs.set_str(STATUS_LIVE, &form)?;
            }
            None => {
                let _ = nvs.remove(STATUS_LIVE);
            }
        }
        Ok(())
    }
}

impl Library for Settings {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        let mut buf;
        {
            let tpl = self.tpl.lock().unwrap();
            let Ok(Some(len)) = tpl.str_len(name) else { return false; };
            buf = vec![0u8; len];
            if tpl.get_str(name, &mut buf).ok().flatten().is_none() {
                return false;
            }
        }
        let text = match std::ffi::CStr::from_bytes_until_nul(&buf) {
            Ok(c) => match c.to_str() {
                Ok(s) => s,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        with(text);
        true
    }
}
