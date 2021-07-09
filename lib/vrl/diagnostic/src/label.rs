use crate::Span;
use codespan_reporting::diagnostic;

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Label {
    pub message: String,
    pub primary: bool,
    pub span: Span,
}

impl Label {
    pub fn primary(message: impl ToString, span: impl Into<Span>) -> Self {
        Self {
            message: message.to_string(),
            primary: true,
            span: span.into(),
        }
    }

    pub fn context(message: impl ToString, span: impl Into<Span>) -> Self {
        Self {
            message: message.to_string(),
            primary: false,
            span: span.into(),
        }
    }
}

impl From<Label> for diagnostic::Label<()> {
    fn from(label: Label) -> Self {
        let style = match label.primary {
            true => diagnostic::LabelStyle::Primary,
            false => diagnostic::LabelStyle::Secondary,
        };

        diagnostic::Label {
            style,
            file_id: (),
            range: label.span.start()..label.span.end(),
            message: label.message,
        }
    }
}
