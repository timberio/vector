use indoc::indoc;
use vrl::{value, Value};
use vrl_wasm::{resolve, ErrorResult, Input, VrlCompileResult};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

#[wasm_bindgen_test]
fn vrl_resolve_success() {
    let cases: Vec<(Value, &str, Value, Value)> = vec![
        (value!({}), ".", value!({}), value!({})),
        (
            value!({"bar": "baz"}),               // Input event
            r#".foo = "bar""#,                    // Program
            value!({"foo": "bar", "bar": "baz"}), // Expected output event
            value!("bar"),                        // Expected output
        ),
        (
            value!({"number": 37, "boolean": true, "quote": "testing"}),
            indoc! {r#"
                del(.number)
                .quote = upcase(string!(.quote))
                .boolean = false
            "#},
            value!({"boolean": false, "quote": "TESTING"}),
            value!(false),
        ),
    ];

    for (event, program, expected_event, expected_output) in cases {
        let input = Input::new(program, event);
        let input_js = JsValue::from_serde(&input).unwrap();
        let result_js = resolve(&input_js);
        let result: VrlCompileResult = result_js.into_serde().unwrap();
        assert_eq!(result.result, expected_event);
        assert_eq!(result.output, expected_output);
    }
}

// We can revisit these tests when we have a less cumbersome way to validate error output
#[wasm_bindgen_test]
fn vrl_resolve_failure() {
    let cases: Vec<(Value, &str, &str)> = vec![
        (value!({}), "1/0", "\nerror[E100]: unhandled error\n  ┌─ :1:1\n  │\n1 │ 1/0\n  │ ^^^\n  │ │\n  │ expression can result in runtime error\n  │ handle the error case to ensure runtime success\n  │\n  = see documentation about error handling at https://errors.vrl.dev/#handling\n  = learn more about error code 100 at https://errors.vrl.dev/100\n  = see language documentation at https://vrl.dev\n"),
    ];

    for (event, program, expected_error) in cases {
        let input = Input::new(program, event);
        let input_js = JsValue::from_serde(&input).unwrap();
        let result_js = resolve(&input_js);
        let error: ErrorResult = result_js.into_serde().unwrap();
        assert_eq!(error.0, expected_error.to_owned());
    }
}
