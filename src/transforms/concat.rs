use super::BuildError;
use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, Value},
    internal_events::{ConcatSubstringError, ConcatSubstringSourceMissing},
    transforms::{FunctionTransform, Transform},
};
use regex::bytes::Regex;
use serde::{Deserialize, Serialize};

use lazy_static::lazy_static;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConcatConfig {
    pub target: String,
    pub joiner: Option<String>,
    pub items: Vec<String>,
}

inventory::submit! {
    TransformDescription::new::<ConcatConfig>("concat")
}

impl GenerateConfig for ConcatConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            target: String::new(),
            joiner: None,
            items: Vec::new(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "concat")]
impl TransformConfig for ConcatConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        let joiner: String = match self.joiner.clone() {
            None => " ".into(),
            Some(var) => var,
        };
        let items = self
            .items
            .iter()
            .map(|item| Substring::new(item.to_owned()))
            .collect::<Result<Vec<Substring>, BuildError>>()?;
        Ok(Transform::function(Concat::new(
            self.target.clone(),
            joiner,
            items,
        )))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "concat"
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Substring {
    source: String,
    start: Option<i32>,
    end: Option<i32>,
}

impl Substring {
    fn new(input: String) -> Result<Substring, BuildError> {
        lazy_static! {
            static ref SUBSTR_REGEX: Regex =
                Regex::new(r"^(?P<source>.*?)(?:\[(?P<start>-?[0-9]*)\.\.(?P<end>-?[0-9]*)\])?$")
                    .unwrap();
        }
        let cap = match SUBSTR_REGEX.captures(input.as_bytes()) {
            None => {
                return Err(BuildError::InvalidSubstring {
                    name: "invalid format, use 'source[start..end]' or 'source'".to_string(),
                })
            }
            Some(cap) => cap,
        };

        let source = match cap.name("source") {
            Some(source) => String::from_utf8_lossy(source.as_bytes()).into(),
            None => {
                return Err(BuildError::InvalidSubstring {
                    name: "invalid format, use 'source[start..end]' or 'source'".into(),
                })
            }
        };
        let start = match cap.name("start") {
            None => None,
            Some(var) => match String::from_utf8_lossy(var.as_bytes()).parse() {
                Ok(var) => Some(var),
                Err(_) => None,
            },
        };
        let end = match cap.name("end") {
            None => None,
            Some(var) => match String::from_utf8_lossy(var.as_bytes()).parse() {
                Ok(var) => Some(var),
                Err(_) => None,
            },
        };

        Ok(Self { source, start, end })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Concat {
    target: String,
    joiner: String,
    items: Vec<Substring>,
}

impl Concat {
    pub fn new(target: String, joiner: String, items: Vec<Substring>) -> Self {
        Self {
            target,
            joiner,
            items,
        }
    }
}

impl FunctionTransform<Event> for Concat {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let mut content_vec: Vec<bytes::Bytes> = Vec::new();

        for substring in self.items.iter() {
            if let Some(value) = event.as_log().get(&substring.source) {
                let b = value.as_bytes();
                let start = match substring.start {
                    None => 0,
                    Some(s) => {
                        if s < 0 {
                            (b.len() as i32 + s) as usize
                        } else {
                            s as usize
                        }
                    }
                };
                let end = match substring.end {
                    None => b.len(),
                    Some(e) => {
                        if e < 0 {
                            (b.len() as i32 + e) as usize
                        } else {
                            e as usize
                        }
                    }
                };
                if start >= end {
                    emit!(ConcatSubstringError {
                        condition: "start >= end",
                        source: substring.source.as_ref(),
                        start,
                        end,
                        length: b.len()
                    });
                    return;
                }
                if start > b.len() {
                    emit!(ConcatSubstringError {
                        condition: "start > len",
                        source: substring.source.as_ref(),
                        start,
                        end,
                        length: b.len()
                    });
                    return;
                }
                if end > b.len() {
                    emit!(ConcatSubstringError {
                        condition: "end > len",
                        source: substring.source.as_ref(),
                        start,
                        end,
                        length: b.len()
                    });
                    return;
                }
                content_vec.push(b.slice(start..end));
            } else {
                emit!(ConcatSubstringSourceMissing {
                    source: substring.source.as_ref()
                });
            }
        }

        let content = content_vec.join(self.joiner.as_bytes());
        event.as_mut_log().insert(
            self.target.clone(),
            Value::from(String::from_utf8_lossy(&content).to_string()),
        );

        output.push(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event::Event, transforms::test::transform_one};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<ConcatConfig>();
    }

    #[test]
    fn concat_to_from() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("first", "Hello vector users");
        event.as_mut_log().insert("second", "World");
        let mut expected = event.clone();
        expected.as_mut_log().insert("out", "Hello users");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![
                Substring::new("first[..5]".to_string()).unwrap(),
                Substring::new("first[-5..]".to_string()).unwrap(),
            ],
        );

        let result = transform_one(&mut transform, event).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn concat_full() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("first", "Hello vector users");
        event.as_mut_log().insert("second", "World");
        let mut expected = event.clone();
        expected.as_mut_log().insert("out", "Hello World");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![
                Substring::new("first[..5]".to_string()).unwrap(),
                Substring::new("second".to_string()).unwrap(),
            ],
        );

        let result = transform_one(&mut transform, event).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn concat_mixed() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("first", "Hello vector users");
        event.as_mut_log().insert("second", "World");
        let mut expected = event.clone();
        expected.as_mut_log().insert("out", "W o r l d");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![
                Substring::new("second[..1]".to_string()).unwrap(),
                Substring::new("second[-4..2]".to_string()).unwrap(),
                Substring::new("second[-3..-2]".to_string()).unwrap(),
                Substring::new("second[3..-1]".to_string()).unwrap(),
                Substring::new("second[4..]".to_string()).unwrap(),
            ],
        );

        let result = transform_one(&mut transform, event).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn concat_start_gt_end() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("only", "Hello vector users");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![Substring::new("only[3..1]".to_string()).unwrap()],
        );

        assert!(transform_one(&mut transform, event).is_none());
    }

    #[test]
    fn concat_start_gt_len() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("only", "World");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![Substring::new("only[10..11]".to_string()).unwrap()],
        );

        assert!(transform_one(&mut transform, event).is_none());
    }

    #[test]
    fn concat_end_gt_len() {
        let mut event = Event::from("message");
        event.as_mut_log().insert("only", "World");

        let mut transform = Concat::new(
            "out".into(),
            " ".into(),
            vec![Substring::new("only[..11]".to_string()).unwrap()],
        );

        assert!(transform_one(&mut transform, event).is_none());
    }
}
