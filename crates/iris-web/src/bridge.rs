//! Scripts evaluated inside the wallpaper page.
//!
//! Every value is a JSON literal or a finite number. Project strings are never
//! concatenated into the script source.

use iris_pkg::Project;
use iris_telemetry::TelemetrySnapshot;
use serde_json::{Map, Value};

/// `general.properties`, otherwise the top-level `properties` object, otherwise `{}`.
pub fn user_properties(project: &Project) -> Value {
    if let Some(Value::Object(map)) = project.general.as_ref() {
        if let Some(properties) = map.get("properties") {
            return properties.clone();
        }
    }
    if let Some(properties) = project.extra.get("properties") {
        return properties.clone();
    }
    Value::Object(Map::new())
}

/// The `general` block, or JSON null when the project omitted it.
pub fn general_block(project: &Project) -> Value {
    project.general.clone().unwrap_or(Value::Null)
}

/// Call `_irisAudioListener` with the 64-band spectrum and stash both widths.
///
/// `None` when any band is non-finite.
pub fn audio_script(bands16: &[f32; 16], bands64: &[f32; 64]) -> Option<String> {
    let mut script = String::from("(function(){var b16=");
    push_array(&mut script, bands16)?;
    script.push_str(";var b64=");
    push_array(&mut script, bands64)?;
    script.push_str(
        ";if(typeof window._irisAudioListener===\"function\"){window._irisAudioListener(b64);}if(window.iris){window.iris.audio={bands16:b16,bands64:b64};}})();",
    );
    Some(script)
}

/// User properties, then `{ telemetry: snapshot }`, then the general block.
pub fn telemetry_script(
    snapshot: &TelemetrySnapshot,
    properties: &Value,
    general: &Value,
) -> Option<String> {
    let properties = serde_json::to_string(properties).ok()?;
    let snapshot = serde_json::to_string(snapshot).ok()?;
    let general = serde_json::to_string(general).ok()?;
    Some(format!(
        "(function(){{var props={properties};var snapshot={snapshot};var general={general};var listener=window.wallpaperPropertyListener;if(!listener){{return;}}if(typeof listener.applyUserProperties===\"function\"){{listener.applyUserProperties(props);listener.applyUserProperties({{telemetry:snapshot}});}}if(general!==null&&typeof listener.applyGeneralProperties===\"function\"){{listener.applyGeneralProperties(general);}}}})();"
    ))
}

/// One-shot property push used before the first telemetry sample.
pub fn properties_script(properties: &Value, general: &Value) -> Option<String> {
    let properties = serde_json::to_string(properties).ok()?;
    let general = serde_json::to_string(general).ok()?;
    Some(format!(
        "(function(){{var props={properties};var general={general};var listener=window.wallpaperPropertyListener;if(!listener){{return;}}if(typeof listener.applyUserProperties===\"function\"){{listener.applyUserProperties(props);}}if(general!==null&&typeof listener.applyGeneralProperties===\"function\"){{listener.applyGeneralProperties(general);}}}})();"
    ))
}

/// Synthetic `pointermove`. `None` when a coordinate is non-finite.
pub fn pointer_script(x: f32, y: f32) -> Option<String> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let x = x.clamp(-1.0e6, 1.0e6);
    let y = y.clamp(-1.0e6, 1.0e6);
    Some(format!(
        "(function(){{var x={x:.4};var y={y:.4};var target=document.body||document.documentElement;if(!target||typeof PointerEvent!==\"function\"){{return;}}var ev=new PointerEvent(\"pointermove\",{{clientX:x,clientY:y,bubbles:true,cancelable:true,pointerId:1,pointerType:\"mouse\"}});target.dispatchEvent(ev);if(window.iris){{window.iris.pointer={{x:x,y:y}};}}}})();"
    ))
}

/// Page Visibility stand-in. Assignment and a redefined getter both run.
pub fn visibility_script(hidden: bool) -> String {
    let flag = if hidden { "true" } else { "false" };
    let state = if hidden { "hidden" } else { "visible" };
    format!(
        "(function(){{var hidden={flag};try{{document.hidden=hidden;}}catch(e){{}}try{{Object.defineProperty(document,\"hidden\",{{configurable:true,get:function(){{return hidden;}}}});}}catch(e){{}}try{{Object.defineProperty(document,\"visibilityState\",{{configurable:true,get:function(){{return \"{state}\";}}}});}}catch(e){{}}try{{document.dispatchEvent(new Event(\"visibilitychange\"));}}catch(e){{}}if(window.iris){{window.iris.paused=hidden;}}}})();"
    )
}

fn push_array(out: &mut String, values: &[f32]) -> Option<()> {
    out.push('[');
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return None;
        }
        if index > 0 {
            out.push(',');
        }
        let text = format!("{:.6}", value.clamp(-1.0e6, 1.0e6));
        out.push_str(&text);
    }
    out.push(']');
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_telemetry_and_pointer_scripts_are_finite_json() {
        let mut bands16 = [0.0; 16];
        bands16[0] = 0.25;
        let mut bands64 = [0.0; 64];
        bands64[3] = 0.5;
        let audio = audio_script(&bands16, &bands64).expect("audio");
        assert!(audio.contains("_irisAudioListener"));
        assert!(audio.contains("0.250000"));
        assert!(audio.contains("0.500000"));
        assert!(audio_script(&[f32::NAN; 16], &bands64).is_none());

        let snapshot = TelemetrySnapshot::default();
        let properties = serde_json::json!({"scheme": {"type": "color", "value": "1 0 0"}});
        let general = serde_json::json!({"properties": properties});
        let telemetry = telemetry_script(&snapshot, &properties, &general).expect("telemetry");
        assert!(telemetry.contains("applyUserProperties"));
        assert!(telemetry.contains("telemetry"));
        assert!(telemetry.contains("applyGeneralProperties"));

        let pointer = pointer_script(12.0, 40.0).expect("pointer");
        assert!(pointer.contains("pointermove"));
        assert!(pointer.contains("12.0000"));
        assert!(pointer_script(f32::NAN, 1.0).is_none());
        assert!(pointer_script(1.0, f32::INFINITY).is_none());

        let hidden = visibility_script(true);
        assert!(hidden.contains("document.hidden"));
        assert!(hidden.contains("visibilitychange"));
        assert!(hidden.contains("hidden"));
        assert!(visibility_script(false).contains("visible"));
    }

    #[test]
    fn user_properties_prefer_the_general_block() {
        let project = Project::parse(
            br#"{
                "type": "web",
                "file": "index.html",
                "general": { "properties": { "speed": { "type": "slider", "value": 1 } } },
                "properties": { "ignored": true }
            }"#,
        )
        .expect("project");
        let properties = user_properties(&project);
        assert!(properties.get("speed").is_some());
        assert!(properties.get("ignored").is_none());

        let loose = Project::parse(
            br#"{"type":"web","properties":{"bloom":{"type":"bool","value":true}}}"#,
        )
        .expect("loose");
        assert!(user_properties(&loose).get("bloom").is_some());
    }
}
