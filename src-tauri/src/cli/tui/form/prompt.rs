use crate::prompt::Prompt;

use super::{
    super::app::{EditorKind, EditorState, EditorSubmit},
    FormFocus, FormMode, PromptMetaField, PromptMetaFormState, TextEditSession, TextInput,
};

const DEFAULT_PROMPT_CONTENT: &str = "# Write your prompt here\n";

impl PromptMetaFormState {
    pub fn new(id: String, name: String) -> Self {
        Self::new_with_details(id, name, "", DEFAULT_PROMPT_CONTENT)
    }

    pub fn new_with_details(
        id: String,
        name: String,
        description: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let content = EditorState::new(
            "Prompt content",
            EditorKind::Plain,
            EditorSubmit::PromptEdit { id: id.clone() },
            content.into(),
        );
        let mut form = Self {
            mode: FormMode::Add,
            focus: FormFocus::Fields,
            field_idx: 0,
            text_edit: None,
            field_errors: Vec::new(),
            id: TextInput::new(id),
            name: TextInput::new(name),
            description: TextInput::new(description.into()),
            content,
            initial_snapshot: Default::default(),
        };
        form.capture_initial_snapshot();
        form
    }

    pub fn from_prompt(prompt: &Prompt) -> Self {
        let mut form = Self {
            mode: FormMode::Edit {
                id: prompt.id.clone(),
            },
            focus: FormFocus::Fields,
            field_idx: 0,
            text_edit: None,
            field_errors: Vec::new(),
            id: TextInput::new(prompt.id.clone()),
            name: TextInput::new(prompt.name.clone()),
            description: TextInput::new(prompt.description.clone().unwrap_or_default()),
            content: EditorState::new(
                "Prompt content",
                EditorKind::Plain,
                EditorSubmit::PromptEdit {
                    id: prompt.id.clone(),
                },
                prompt.content.clone(),
            ),
            initial_snapshot: Default::default(),
        };
        form.capture_initial_snapshot();
        form
    }

    fn capture_initial_snapshot(&mut self) {
        self.initial_snapshot = self.snapshot();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.snapshot() != self.initial_snapshot
    }

    pub fn fields(&self) -> Vec<PromptMetaField> {
        vec![
            PromptMetaField::Id,
            PromptMetaField::Name,
            PromptMetaField::Description,
        ]
    }

    pub fn input(&self, field: PromptMetaField) -> &TextInput {
        match field {
            PromptMetaField::Id => &self.id,
            PromptMetaField::Name => &self.name,
            PromptMetaField::Description => &self.description,
        }
    }

    pub fn input_mut(&mut self, field: PromptMetaField) -> &mut TextInput {
        match field {
            PromptMetaField::Id => &mut self.id,
            PromptMetaField::Name => &mut self.name,
            PromptMetaField::Description => &mut self.description,
        }
    }

    pub fn text_edit_target(&self) -> Option<PromptMetaField> {
        self.text_edit.as_ref().map(TextEditSession::target)
    }

    pub fn begin_text_edit(&mut self, field: PromptMetaField) {
        let original = self.input(field).clone();
        let original_error = self.field_error(field).map(str::to_string);
        self.clear_field_error(field);
        self.text_edit = Some(TextEditSession::new(field, original, original_error));
        if let Some(index) = self
            .fields()
            .iter()
            .position(|candidate| *candidate == field)
        {
            self.field_idx = index;
        }
    }

    pub fn take_text_edit(&mut self) -> Option<TextEditSession<PromptMetaField>> {
        self.text_edit.take()
    }

    pub fn cancel_text_edit(&mut self) -> Option<PromptMetaField> {
        let (field, original, original_error) = self.text_edit.take()?.into_parts();
        *self.input_mut(field) = original;
        if let Some(index) = self
            .fields()
            .iter()
            .position(|candidate| *candidate == field)
        {
            self.field_idx = index;
        }
        if let Some(message) = original_error {
            self.set_field_error(field, message);
        } else {
            self.clear_field_error(field);
        }
        Some(field)
    }

    pub fn field_error(&self, field: PromptMetaField) -> Option<&str> {
        self.field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_field_error(&mut self, field: PromptMetaField, message: impl Into<String>) {
        self.clear_field_error(field);
        self.field_errors.push(super::InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_field_error(&mut self, field: PromptMetaField) {
        self.field_errors.retain(|error| error.field != field);
    }

    pub fn id_value(&self) -> String {
        self.id.value.trim().to_string()
    }

    pub fn name_value(&self) -> String {
        self.name.value.trim().to_string()
    }

    pub fn description_value(&self) -> Option<String> {
        let value = self.description.value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    pub fn content_value(&self) -> String {
        self.content.text()
    }

    fn snapshot(&self) -> (String, String, String, String) {
        (
            self.id_value(),
            self.name_value(),
            self.description.value.trim().to_string(),
            self.content.text(),
        )
    }
}
