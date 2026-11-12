use super::*;

#[derive(Default)]
struct Budget {
    text: usize,
    items: usize,
}
impl Budget {
    fn text(&mut self, value: &str, cap: usize) -> Result<(), Invalid> {
        self.text = self.text.saturating_add(value.len());
        if value.len() > cap || self.text > MAX_REPLY_TEXT {
            return Err(Invalid("Compose text"));
        }
        Ok(())
    }
    fn strings(&mut self, values: &[String]) -> Result<(), Invalid> {
        self.items = self.items.saturating_add(values.len());
        if self.items > MAX_ITEMS {
            return Err(Invalid("Compose metadata count"));
        }
        values.iter().try_for_each(|v| self.text(v, MAX_TEXT))
    }
}
impl Project {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.files.len() > MAX_FILES || self.config_files.len() > MAX_FILES {
            return Err(Invalid("Compose file count"));
        }
        if self.cli.is_some() && self.cli_error.is_some()
            || self.services.is_some() && self.config_error.is_some()
        {
            return Err(Invalid("Compose project outcome"));
        }
        let mut budget = Budget::default();
        if let Some(cli) = &self.cli {
            budget.text(&cli.version, MAX_TEXT)?;
        }
        for v in self
            .cli_error
            .iter()
            .chain(&self.working_dir)
            .chain(&self.config_error)
        {
            budget.text(v, MAX_TEXT)?;
        }
        budget.strings(&self.config_files)?;
        for file in &self.files {
            budget.text(&file.path, MAX_TEXT)?;
            if let Some(text) = &file.text {
                budget.text(text, MAX_FILE_TEXT)?;
            }
            if let Some(error) = &file.error {
                budget.text(error, MAX_TEXT)?;
            }
        }
        let services = self.services.as_deref().unwrap_or_default();
        if services.len() > MAX_ITEMS {
            return Err(Invalid("Compose service count"));
        }
        budget.items = budget.items.saturating_add(services.len());
        let mut names = std::collections::BTreeSet::new();
        for service in services {
            if !valid_service(&service.name) || !names.insert(&service.name) {
                return Err(Invalid("Compose service name"));
            }
            budget.text(&service.name, MAX_TEXT)?;
            if let Some(image) = &service.image {
                budget.text(image, MAX_TEXT)?;
            }
            budget.strings(&service.profiles)?;
        }
        Ok(())
    }
}
impl Job {
    fn validate_with(&self, budget: &mut Budget) -> Result<(), Invalid> {
        if !valid_job(&self.job) || !valid_project(&self.project) {
            return Err(Invalid("Compose job identity"));
        }
        let consistent = match self.state {
            State::Running => self.exit_code.is_none() && self.finished_at.is_none(),
            State::Succeeded => self.exit_code == Some(0) && self.finished_at.is_some(),
            State::Failed => self.exit_code != Some(0) && self.finished_at.is_some(),
            State::Cancelled => self.finished_at.is_some(),
        };
        if !consistent || self.output.len() as u64 > self.offset {
            return Err(Invalid("Compose job progress"));
        }
        for v in [&self.job, &self.project, &self.command] {
            budget.text(v, MAX_TEXT)?;
        }
        budget.text(&self.output, MAX_OUTPUT)
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        self.validate_with(&mut Budget::default())
    }
}
impl Jobs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.jobs.len() > MAX_JOBS {
            return Err(Invalid("Compose job count"));
        }
        let mut budget = Budget::default();
        let mut ids = std::collections::BTreeSet::new();
        for job in &self.jobs {
            if !ids.insert(&job.job) || !job.output.is_empty() || job.offset != 0 {
                return Err(Invalid("Compose job listing"));
            }
            job.validate_with(&mut budget)?;
        }
        Ok(())
    }
}
