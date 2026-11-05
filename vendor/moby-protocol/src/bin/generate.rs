use std::{env, error::Error, fs, path::Path};

use moby_protocol::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, ApiError, ChatRequest, ChatResponse,
    ConsentType, Expression, Language, LocalTime, PACKAGE_VERSION, PROTOCOL_VERSION, ReminderText,
    ResourceKind, Script, SearchLimit, SearchQuery, StableId, ToolCall, ToolFailureCode, ToolName,
    ToolRequest, ToolResource, ToolResult, UsageInfo, ZonedDateTime, can_transition, usb,
};
use schemars::{JsonSchema, Schema, generate::SchemaSettings, transform::RecursiveTransform};
use serde_json::{Value, json};
use ts_rs::{Config, TS};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn schema<T: JsonSchema>() -> Result<String> {
    // Deserialization includes optional/null fields accepted by serde.
    let mut schema = SchemaSettings::draft2020_12()
        .for_deserialize()
        .with_transform(RecursiveTransform(|schema: &mut Schema| {
            // Bounds express these integers without validator-specific formats.
            if matches!(
                schema.get("format").and_then(Value::as_str),
                Some("uint8" | "uint16" | "uint32")
            ) {
                schema.remove("format");
            }
        }))
        .into_generator()
        .into_root_schema_for::<T>();
    schema.insert("x-package-version".into(), json!(PACKAGE_VERSION));
    Ok(format!("{}\n", serde_json::to_string_pretty(&schema)?))
}

/// C definitions derived from the Rust types: field bounds, message names,
/// error codes, and the expression transition table.
fn c_protocol_definitions() -> Result<String> {
    use std::fmt::Write;

    let mut text = String::new();
    let int_bounds: [(&str, u64, u64); 11] = [
        (
            "AUDIO_STREAM_ID",
            usb::AudioStreamId::MIN.into(),
            usb::AudioStreamId::MAX.into(),
        ),
        (
            "ACTION_DURATION_MS",
            usb::ActionDurationMs::MIN.into(),
            usb::ActionDurationMs::MAX.into(),
        ),
        (
            "TEXT_TTL_MS",
            usb::TextTtlMs::MIN.into(),
            usb::TextTtlMs::MAX.into(),
        ),
        (
            "PAIR_TIMEOUT_MS",
            usb::PairTimeoutMs::MIN.into(),
            usb::PairTimeoutMs::MAX.into(),
        ),
        (
            "SAMPLE_RATE",
            usb::SampleRate::MIN.into(),
            usb::SampleRate::MAX.into(),
        ),
        (
            "CHANNEL_COUNT",
            usb::ChannelCount::MIN.into(),
            usb::ChannelCount::MAX.into(),
        ),
        (
            "FRAME_MS",
            usb::FrameMs::MIN.into(),
            usb::FrameMs::MAX.into(),
        ),
        (
            "PREROLL_MS",
            usb::PrerollMs::MIN.into(),
            usb::PrerollMs::MAX.into(),
        ),
        (
            "PERCENT",
            usb::Percent::MIN.into(),
            usb::Percent::MAX.into(),
        ),
        (
            "VAD_SILENCE_MS",
            usb::VadSilenceMs::MIN.into(),
            usb::VadSilenceMs::MAX.into(),
        ),
        (
            "MAX_UTTERANCE_MS",
            usb::MaxUtteranceMs::MIN.into(),
            usb::MaxUtteranceMs::MAX.into(),
        ),
    ];
    text.push_str(
        "
/* Inclusive field bounds. Text bounds count code points, up to 4 UTF-8 bytes each. */
",
    );
    for (name, min, max) in int_bounds {
        writeln!(text, "#define MOBY_USB_{name}_MIN {min}u")?;
        writeln!(text, "#define MOBY_USB_{name}_MAX {max}u")?;
    }
    let text_bounds = [
        (
            "SCREEN_TEXT",
            usb::ScreenText::MIN_CHARS,
            usb::ScreenText::MAX_CHARS,
        ),
        (
            "ERROR_DETAIL",
            usb::ErrorDetail::MIN_CHARS,
            usb::ErrorDetail::MAX_CHARS,
        ),
        (
            "VERSION_TEXT",
            usb::VersionText::MIN_CHARS,
            usb::VersionText::MAX_CHARS,
        ),
    ];
    for (name, min, max) in text_bounds {
        writeln!(text, "#define MOBY_USB_{name}_MIN_CHARS {min}u")?;
        writeln!(text, "#define MOBY_USB_{name}_MAX_CHARS {max}u")?;
    }
    writeln!(
        text,
        "#define MOBY_USB_CAPABILITIES_MAX {}u",
        usb::Capabilities::MAX
    )?;
    writeln!(text, "#define MOBY_USB_NONCE_BYTES {}u", usb::Nonce::BYTES)?;
    writeln!(text, "#define MOBY_USB_PROOF_BYTES {}u", usb::Proof::BYTES)?;
    writeln!(
        text,
        "#define MOBY_USB_PAIR_KEY_BYTES {}u",
        usb::PairKey::BYTES
    )?;

    text.push_str(
        "
/* Control message names (the envelope `name`). */
",
    );
    for info in usb::MESSAGES {
        writeln!(
            text,
            "#define MOBY_USB_MSG_{} {:?} /* {}, from the {} */",
            info.name.to_ascii_uppercase(),
            info.name,
            info.kind,
            info.sender.as_str()
        )?;
    }

    text.push_str(
        "
/* Error codes in `error` events and `command_error` responses. */
",
    );
    for code in usb::ErrorCode::ALL {
        let wire = serde_json::to_string(&code)?;
        writeln!(text, "#define MOBY_USB_{} {wire}", wire.trim_matches('"'))?;
    }

    text.push_str(
        "
/* Face states in the order of the transition table below. */
typedef enum moby_expression {
",
    );
    for (index, expression) in Expression::ALL.iter().enumerate() {
        let wire = serde_json::to_string(expression)?;
        writeln!(
            text,
            "    MOBY_EXPRESSION_{} = {index},",
            wire.trim_matches('"').to_ascii_uppercase()
        )?;
    }
    writeln!(
        text,
        "    MOBY_EXPRESSION_COUNT = {}
}} moby_expression;",
        Expression::ALL.len()
    )?;
    text.push_str(
        "
static inline const char *moby_expression_name(moby_expression state) {
    static const char *const names[MOBY_EXPRESSION_COUNT] = {
",
    );
    for expression in Expression::ALL {
        writeln!(text, "        {},", serde_json::to_string(&expression)?)?;
    }
    text.push_str(
        "    };
    return (unsigned)state < MOBY_EXPRESSION_COUNT ? names[state] : \"\";
}
",
    );

    text.push_str(
        "
/* Row = old state, bit = new state. Generated from Rust can_transition. */
static inline bool moby_can_transition(moby_expression from, moby_expression to) {
    static const unsigned short rows[MOBY_EXPRESSION_COUNT] = {
",
    );
    for from in Expression::ALL {
        let mask = Expression::ALL
            .iter()
            .enumerate()
            .filter(|&(_, &to)| can_transition(from, to))
            .fold(0u16, |mask, (index, _)| mask | 1 << index);
        writeln!(
            text,
            "        0x{mask:04x}u, /* {} */",
            serde_json::to_string(&from)?.trim_matches('"')
        )?;
    }
    text.push_str(
        "    };
    return (unsigned)from < MOBY_EXPRESSION_COUNT && (unsigned)to < MOBY_EXPRESSION_COUNT &&
           ((rows[from] >> to) & 1u) != 0;
}
",
    );
    Ok(text)
}

fn artifacts() -> Result<Vec<(String, String)>> {
    let mut outputs = Vec::new();
    let mut typescript =
        String::from("// Generated from Rust. Run npm run generate; do not edit.\n\n");
    let config = Config::new();
    macro_rules! export {
        ($ty:ty) => {{
            typescript.push_str("export ");
            typescript.push_str(&<$ty>::decl(&config));
            typescript.push_str("\n\n");
        }};
        ($ty:ty, $name:literal) => {{
            outputs.push((format!("schema/{}.schema.json", $name), schema::<$ty>()?));
            export!($ty);
        }};
    }
    export!(StableId, "stable-id");
    export!(Expression, "expression");
    export!(ZonedDateTime);
    export!(SearchQuery);
    export!(SearchLimit);
    export!(ReminderText);
    export!(ToolName);
    export!(ChatRequest, "chat-request");
    export!(ToolRequest, "tool-request");
    export!(ToolCall);
    export!(ToolFailureCode);
    export!(ToolResult, "tool-result");
    export!(ChatResponse, "chat-response");
    export!(ApiError, "api-error");
    export!(Language, "language");
    export!(Script, "script");
    export!(LocalTime);
    export!(ConsentType, "consent-type");
    export!(UsageInfo, "usage-info");
    export!(ResourceKind);
    export!(ToolResource);
    export!(AgentMessage);
    export!(AgentTurnRequest, "agent-turn-request");
    export!(AgentTurnResponse, "agent-turn-response");
    export!(usb::ControlVersion);
    export!(usb::EventKind);
    export!(usb::CommandKind);
    export!(usb::ResponseKind);
    export!(usb::Empty);
    export!(usb::ErrorCode);
    export!(usb::ErrorDetail);
    export!(usb::VersionText);
    export!(usb::ScreenText);
    export!(usb::AudioStreamId);
    export!(usb::ActionDurationMs);
    export!(usb::TextTtlMs);
    export!(usb::PairTimeoutMs);
    export!(usb::SampleRate);
    export!(usb::ChannelCount);
    export!(usb::FrameMs);
    export!(usb::PrerollMs);
    export!(usb::Percent);
    export!(usb::VadSilenceMs);
    export!(usb::MaxUtteranceMs);
    export!(usb::Nonce);
    export!(usb::Proof);
    export!(usb::PairKey);
    export!(usb::Capabilities);
    export!(usb::ErrorInfo);
    export!(usb::DeviceConfig);
    export!(usb::Hello);
    export!(usb::HelloAck);
    export!(usb::Heartbeat);
    export!(usb::WakeDetected);
    export!(usb::Button);
    export!(usb::ButtonAction);
    export!(usb::ButtonEvent);
    export!(usb::MuteSource);
    export!(usb::MuteChanged);
    export!(usb::AudioStartReason);
    export!(usb::AudioCodec);
    export!(usb::AudioStart);
    export!(usb::AudioEndReason);
    export!(usb::AudioEnd);
    export!(usb::SetExpression);
    export!(usb::DisplayText);
    export!(usb::AudioPlay);
    export!(usb::Cancel);
    export!(usb::ServoPose);
    export!(usb::PairBegin);
    export!(usb::PairSetKey);
    export!(usb::ControlMessage, "usb-control");

    typescript.push_str(&format!(
        "export const PACKAGE_VERSION = {PACKAGE_VERSION:?} as const;\n\n"
    ));
    typescript.push_str(&format!(
        "export const PROTOCOL_VERSION = {PROTOCOL_VERSION} as const;\n\n"
    ));
    let usb_constants = [
        ("FRAME_VERSION", usb::FRAME_VERSION.into()),
        ("DELIMITER", usb::DELIMITER.into()),
        ("HEADER_LEN", usb::HEADER_LEN),
        ("CRC_LEN", usb::CRC_LEN),
        ("MAX_PAYLOAD_LEN", usb::MAX_PAYLOAD_LEN),
        ("MAX_DECODED_LEN", usb::MAX_DECODED_LEN),
        ("MAX_ENCODED_LEN", usb::MAX_ENCODED_LEN),
        ("MAX_WIRE_LEN", usb::MAX_WIRE_LEN),
        ("VERSION_OFFSET", usb::VERSION_OFFSET),
        ("KIND_OFFSET", usb::KIND_OFFSET),
        ("FLAGS_OFFSET", usb::FLAGS_OFFSET),
        ("RESERVED_OFFSET", usb::RESERVED_OFFSET),
        ("STREAM_ID_OFFSET", usb::STREAM_ID_OFFSET),
        ("SEQUENCE_OFFSET", usb::SEQUENCE_OFFSET),
        ("LENGTH_OFFSET", usb::LENGTH_OFFSET),
        ("CONTROL", usb::FrameKind::Control as usize),
        ("ROBOT_AUDIO", usb::FrameKind::RobotAudio as usize),
        ("DESKTOP_AUDIO", usb::FrameKind::DesktopAudio as usize),
    ];
    let mut usb_header = format!(
        "/* Generated from Rust. Run npm run generate. */\n\
         #ifndef MOBY_USB_H\n#define MOBY_USB_H\n\n\
         #define MOBY_USB_PROTOCOL_VERSION {PROTOCOL_VERSION}u\n"
    );
    typescript.push_str("export const USB = {\n");
    for (name, value) in usb_constants {
        typescript.push_str(&format!("  {name}: {value},\n"));
        usb_header.push_str(&format!("#define MOBY_USB_{name} {value}u\n"));
    }
    typescript.push_str("} as const;\n\n");
    typescript.push_str("export const CONTROL_MESSAGES = [\n");
    for info in usb::MESSAGES {
        typescript.push_str(&format!(
            "  {{ name: {:?}, kind: {:?}, sender: {:?} }},\n",
            info.name,
            info.kind,
            info.sender.as_str()
        ));
    }
    typescript.push_str("] as const;\n\n");
    let error_codes = usb::ErrorCode::ALL
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    typescript.push_str(&format!(
        "export const ERROR_CODES = [{}] as const satisfies readonly ErrorCode[];\n\n",
        error_codes.join(", ")
    ));
    usb_header.push_str(&c_protocol_definitions()?);
    usb_header.push_str("\n#endif /* MOBY_USB_H */\n");
    outputs.push(("include/moby_usb.h".into(), usb_header));

    let expressions = Expression::ALL
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    typescript.push_str(&format!(
        "export const EXPRESSIONS = [{}] as const satisfies readonly Expression[];\n\n",
        expressions.join(", ")
    ));
    typescript
        .push_str("const transitions: Readonly<Record<Expression, readonly Expression[]>> = {\n");
    for from in Expression::ALL {
        let allowed = Expression::ALL
            .iter()
            .filter(|&&to| can_transition(from, to))
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        typescript.push_str(&format!(
            "  {}: [{}],\n",
            serde_json::to_string(&from)?,
            allowed.join(", ")
        ));
    }
    typescript.push_str(
        "};\n\nexport function canTransition(from: Expression, to: Expression): boolean {\n  return transitions[from].includes(to);\n}\n",
    );
    outputs.push(("generated/index.ts".into(), typescript));
    Ok(outputs)
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let check = match args.as_slice() {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: generate [--check]".into()),
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let outputs = artifacts()?;
    let mut stale = false;
    for (relative, expected) in &outputs {
        let path = root.join(relative);
        if check {
            // Compare bytes so CRLF drift cannot silently differ from releases.
            if fs::read(&path).ok().as_deref() != Some(expected.as_bytes()) {
                eprintln!("out of date: {relative}");
                stale = true;
            }
        } else {
            fs::create_dir_all(path.parent().ok_or("missing artifact parent")?)?;
            fs::write(path, expected)?;
            println!("generated {relative}");
        }
    }
    // Renamed or removed types must not leave obsolete schema exports behind.
    if let Ok(entries) = fs::read_dir(root.join("schema")) {
        for entry in entries {
            let entry = entry?;
            let relative = format!("schema/{}", entry.file_name().to_string_lossy());
            if !outputs.iter().any(|(name, _)| name == &relative) {
                eprintln!("unexpected artifact (remove explicitly): {relative}");
                stale = true;
            }
        }
    }
    if stale {
        return Err("generated files differ; run npm run generate and commit the results".into());
    }
    if check {
        println!("all {} generated files are current", outputs.len());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("generation failed: {error}");
        std::process::exit(1);
    }
}
