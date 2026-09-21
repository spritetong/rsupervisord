// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::compat::xmlrpc::fault::{Fault, FaultCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::BTreeMap;

/// Dynamically typed XML-RPC value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    Boolean(bool),
    String(String),
    Double(f64),
    DateTime(String),
    Base64(Vec<u8>),
    Array(Vec<Value>),
    Struct(BTreeMap<String, Value>),
    Nil,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Double(d) => Some(*d as i32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Struct(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Base64(b) => Some(b.as_slice()),
            Value::String(s) => Some(s.as_bytes()),
            _ => None,
        }
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Boolean(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}

impl From<BTreeMap<String, Value>> for Value {
    fn from(v: BTreeMap<String, Value>) -> Self {
        Value::Struct(v)
    }
}

/// Parsed incoming XML-RPC method call.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodCall {
    pub name: String,
    pub params: Vec<Value>,
}

impl MethodCall {
    /// Parses an incoming XML-RPC string into a MethodCall.
    pub fn parse(xml: &str) -> Result<Self, Fault> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut method_name = None;
        let mut params = Vec::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let tag = e.name();
                    let tag_str = tag.as_ref();
                    match tag_str {
                        "methodName" => {
                            let name = reader
                                .read_text(e.name())
                                .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                            method_name = Some(name.to_string());
                        }
                        "params" => {
                            params = parse_params(&mut reader)?;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(Fault::incorrect_params(&format!("Malformed XML: {}", e))),
                _ => {}
            }
            buf.clear();
        }

        let name = method_name
            .ok_or_else(|| Fault::new(FaultCode::IncorrectParameters, "Missing methodName tag"))?;

        // CVE-2017-11610 security validation:
        // Must contain exactly one dot (2 segments), not start with '_' or contain illegal characters.
        validate_method_name(&name)?;

        Ok(Self { name, params })
    }
}

/// Validates that method name conforms strictly to 2-segment dot-separated convention (CVE-2017-11610).
pub fn validate_method_name(name: &str) -> Result<(), Fault> {
    if name.starts_with('_') || name.contains("..") {
        return Err(Fault::unknown_method(name));
    }
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(Fault::unknown_method(name));
    }
    if parts[0].starts_with('_') || parts[1].starts_with('_') {
        return Err(Fault::unknown_method(name));
    }
    Ok(())
}

fn parse_params(reader: &mut Reader<&[u8]>) -> Result<Vec<Value>, Fault> {
    let mut params = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == "param" => {
                let val = parse_param_value(reader)?;
                params.push(val);
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "params" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }
    Ok(params)
}

fn parse_param_value(reader: &mut Reader<&[u8]>) -> Result<Value, Fault> {
    let mut buf = Vec::new();
    let mut found_value = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == "value" => {
                found_value = Some(parse_value_element(reader)?);
            }
            Ok(Event::Empty(ref e)) if e.name().as_ref() == "value" => {
                found_value = Some(Value::String(String::new()));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "param" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    found_value.ok_or_else(|| Fault::incorrect_params("param without value"))
}

fn parse_value_element(reader: &mut Reader<&[u8]>) -> Result<Value, Fault> {
    let mut buf = Vec::new();
    let mut current_val = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = e.name();
                match tag.as_ref() {
                    "i4" | "int" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        let n = txt
                            .trim()
                            .parse::<i32>()
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        current_val = Some(Value::Int(n));
                    }
                    "boolean" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        let b = match txt.trim() {
                            "1" | "true" => true,
                            "0" | "false" => false,
                            _ => return Err(Fault::incorrect_params("Invalid boolean value")),
                        };
                        current_val = Some(Value::Boolean(b));
                    }
                    "string" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        current_val = Some(Value::String(txt.to_string()));
                    }
                    "double" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        let d = txt
                            .trim()
                            .parse::<f64>()
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        current_val = Some(Value::Double(d));
                    }
                    "dateTime.iso8601" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        current_val = Some(Value::DateTime(txt.to_string()));
                    }
                    "base64" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        let clean: String = txt.chars().filter(|c| !c.is_whitespace()).collect();
                        let decoded = BASE64_STANDARD
                            .decode(clean)
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        current_val = Some(Value::Base64(decoded));
                    }
                    "struct" => {
                        let s = parse_struct(reader)?;
                        current_val = Some(Value::Struct(s));
                    }
                    "array" => {
                        let a = parse_array(reader)?;
                        current_val = Some(Value::Array(a));
                    }
                    "nil" => {
                        current_val = Some(Value::Nil);
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = e.name();
                if tag.as_ref() == "nil" {
                    current_val = Some(Value::Nil);
                } else if tag.as_ref() == "string" {
                    current_val = Some(Value::String(String::new()));
                }
            }
            Ok(Event::Text(ref e)) => {
                // Untagged string value directly inside <value>text</value>
                if current_val.is_none() {
                    let txt = quick_xml::escape::unescape(e.as_ref())
                        .map_err(|err| Fault::incorrect_params(&err.to_string()))?;
                    current_val = Some(Value::String(txt.to_string()));
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "value" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(current_val.unwrap_or(Value::String(String::new())))
}

fn parse_struct(reader: &mut Reader<&[u8]>) -> Result<BTreeMap<String, Value>, Fault> {
    let mut map = BTreeMap::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == "member" => {
                let (k, v) = parse_member(reader)?;
                map.insert(k, v);
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "struct" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&e.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(map)
}

fn parse_member(reader: &mut Reader<&[u8]>) -> Result<(String, Value), Fault> {
    let mut buf = Vec::new();
    let mut key = None;
    let mut val = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = e.name();
                match tag.as_ref() {
                    "name" => {
                        let txt = reader
                            .read_text(e.name())
                            .map_err(|e| Fault::incorrect_params(&e.to_string()))?;
                        key = Some(txt.to_string());
                    }
                    "value" => {
                        val = Some(parse_value_element(reader)?);
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) if e.name().as_ref() == "value" => {
                val = Some(Value::String(String::new()));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "member" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let k = key.ok_or_else(|| Fault::incorrect_params("struct member missing name"))?;
    let v = val.unwrap_or(Value::String(String::new()));
    Ok((k, v))
}

fn parse_array(reader: &mut Reader<&[u8]>) -> Result<Vec<Value>, Fault> {
    let mut items = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == "value" => {
                items.push(parse_value_element(reader)?);
            }
            Ok(Event::Empty(ref e)) if e.name().as_ref() == "value" => {
                items.push(Value::String(String::new()));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == "array" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Fault::incorrect_params(&e.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(items)
}

/// Serializes an XML-RPC result or fault into standard wire XML.
pub fn serialize_response(res: Result<Value, Fault>) -> String {
    match res {
        Ok(val) => {
            let mut out =
                String::from("<?xml version=\"1.0\"?>\n<methodResponse>\n<params>\n<param>\n");
            serialize_value(&val, &mut out);
            out.push_str("\n</param>\n</params>\n</methodResponse>\n");
            out
        }
        Err(fault) => {
            let mut out = String::from(
                "<?xml version=\"1.0\"?>\n<methodResponse>\n<fault>\n<value>\n<struct>\n",
            );
            out.push_str("<member>\n<name>faultCode</name>\n<value><int>");
            out.push_str(&fault.code.to_string());
            out.push_str("</int></value>\n</member>\n");
            out.push_str("<member>\n<name>faultString</name>\n<value><string>");
            escape_xml(&fault.message, &mut out);
            out.push_str("</string></value>\n</member>\n");
            out.push_str("</struct>\n</value>\n</fault>\n</methodResponse>\n");
            out
        }
    }
}

pub fn serialize_value(val: &Value, out: &mut String) {
    match val {
        Value::Int(i) => {
            out.push_str("<value><int>");
            out.push_str(&i.to_string());
            out.push_str("</int></value>");
        }
        Value::Boolean(b) => {
            out.push_str("<value><boolean>");
            out.push_str(if *b { "1" } else { "0" });
            out.push_str("</boolean></value>");
        }
        Value::String(s) => {
            out.push_str("<value><string>");
            escape_xml(s, out);
            out.push_str("</string></value>");
        }
        Value::Double(d) => {
            out.push_str("<value><double>");
            out.push_str(&d.to_string());
            out.push_str("</double></value>");
        }
        Value::DateTime(dt) => {
            out.push_str("<value><dateTime.iso8601>");
            escape_xml(dt, out);
            out.push_str("</dateTime.iso8601></value>");
        }
        Value::Base64(b) => {
            out.push_str("<value><base64>");
            out.push_str(&BASE64_STANDARD.encode(b));
            out.push_str("</base64></value>");
        }
        Value::Array(items) => {
            out.push_str("<value><array><data>\n");
            for item in items {
                serialize_value(item, out);
                out.push('\n');
            }
            out.push_str("</data></array></value>");
        }
        Value::Struct(members) => {
            out.push_str("<value><struct>\n");
            for (k, v) in members {
                out.push_str("<member>\n<name>");
                escape_xml(k, out);
                out.push_str("</name>\n");
                serialize_value(v, out);
                out.push_str("\n</member>\n");
            }
            out.push_str("</struct></value>");
        }
        Value::Nil => {
            out.push_str("<value><nil/></value>");
        }
    }
}

fn escape_xml(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_method_call() {
        let xml = r#"<?xml version="1.0"?>
        <methodCall>
          <methodName>supervisor.getAPIVersion</methodName>
          <params></params>
        </methodCall>"#;
        let call = MethodCall::parse(xml).expect("parse simple");
        assert_eq!(call.name, "supervisor.getAPIVersion");
        assert!(call.params.is_empty());
    }

    #[test]
    fn test_parse_method_call_with_string_param() {
        let xml = r#"<?xml version="1.0"?>
        <methodCall>
          <methodName>supervisor.getProcessInfo</methodName>
          <params>
            <param>
              <value><string>services:ticker</string></value>
            </param>
          </params>
        </methodCall>"#;
        let call = MethodCall::parse(xml).expect("parse param");
        assert_eq!(call.name, "supervisor.getProcessInfo");
        assert_eq!(call.params.len(), 1);
        assert_eq!(call.params[0], Value::String("services:ticker".to_string()));
    }

    #[test]
    fn test_parse_untagged_value_as_string() {
        let xml = r#"<methodCall>
          <methodName>supervisor.startProcess</methodName>
          <params>
            <param><value>ticker</value></param>
            <param><value><boolean>1</boolean></value></param>
          </params>
        </methodCall>"#;
        let call = MethodCall::parse(xml).expect("parse untagged");
        assert_eq!(call.params.len(), 2);
        assert_eq!(call.params[0], Value::String("ticker".to_string()));
        assert_eq!(call.params[1], Value::Boolean(true));
    }

    #[test]
    fn test_cve_method_name_validation() {
        assert!(
            MethodCall::parse("<methodCall><methodName>_private</methodName></methodCall>")
                .is_err()
        );
        assert!(
            MethodCall::parse(
                "<methodCall><methodName>supervisor.sub.method</methodName></methodCall>"
            )
            .is_err()
        );
        assert!(
            MethodCall::parse("<methodCall><methodName>supervisor</methodName></methodCall>")
                .is_err()
        );
        assert!(
            MethodCall::parse(
                "<methodCall><methodName>supervisor._start</methodName></methodCall>"
            )
            .is_err()
        );
        assert!(
            MethodCall::parse(
                "<methodCall><methodName>supervisor.startProcess</methodName></methodCall>"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_serialize_response_and_fault() {
        let res_xml = serialize_response(Ok(Value::String("3.0".to_string())));
        assert!(res_xml.contains("<methodResponse>"));
        assert!(res_xml.contains("<string>3.0</string>"));

        let fault = Fault::unknown_method("foo.bar");
        let fault_xml = serialize_response(Err(fault));
        assert!(fault_xml.contains("<fault>"));
        assert!(fault_xml.contains("<int>1</int>"));
        assert!(fault_xml.contains("UNKNOWN_METHOD"));
    }
}
