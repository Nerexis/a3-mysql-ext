# Optional Arma extension features

The standard extDB3-compatible `2:` request mode and its `4:`/`5:` polling behavior are unchanged. This extension also offers an opt-in callback mode for integrations that can install Arma's `ExtensionCallback` mission event handler.

## Callback result mode

Call the same SQL request with prefix `6:`. For example, `6:SQL:slow:async` starts an asynchronous request and returns the usual `[2,"<request id>"]` ticket. The extension sends an `ExtensionCallback` event with:

- name: `extDB3`
- function: `asyncResultReady`
- data: a parseable array containing the request ID, for example `[42]`

The result remains in the regular result store. The callback is only a ready notification; retrieve the response once with `4:<id>`. If that returns `[5]`, retrieve fragments with `5:<id>` until it returns an empty string. The result is also available through the regular polling path if a callback is lost or the event handler is not present. A `6:` request made before the extension has received a callback registration returns `[0,"Error Callback Unavailable"]`.

Install the mission event handler before issuing `6:` requests. One possible server-side pattern is:

```sqf
addMissionEventHandler ["ExtensionCallback", {
    params ["_name", "_function", "_data"];
    if (_name isEqualTo "extDB3" && _function isEqualTo "asyncResultReady") then {
        private _requestId = (parseSimpleArray _data) select 0;
        private _result = ("extDB3" callExtension ["4", [str _requestId]]) select 0;
        if (_result isEqualTo "[5]") then {
            private _parts = "";
            private _part = "";
            do {
                _part = ("extDB3" callExtension ["5", [str _requestId]]) select 0;
                _parts = _parts + _part;
            } while {!(_part isEqualTo "")};
            _result = _parts;
        };
        // Handle _result here.
    };
}];
```

The callback buffer has a per-frame limit. If Arma reports that it is full, the extension retries the notification while retaining the result. Callbacks carry only the ID, so large results use the same multipart mechanism as polling. A mission restart clears Arma's callback buffer; pending results remain retrievable only while the extension process and request result are alive. See the [Arma extension interface documentation](https://community.bistudio.com/wiki/Extensions) for the callback ABI and buffer semantics.

Callback mode is an extension-specific addition. It does not change the documented extDB3 request formats.

## Arma 3 2.22 raw `RVExtensionArgs` strings

By default, `RVExtensionArgs` retains extDB3-compatible handling for escaped strings. For Arma 3 2.22 or newer, build with the `arma-2.22-no-escape` Cargo feature. That build exports `RVExtensionFeatureFlags` with `RVFeature_ArgumentNoEscapeString` set and passes string arguments through without unquoting them. Do not use that build with older Arma versions. The feature flag and version requirement are documented in the [Arma extension interface](https://community.bistudio.com/wiki/Extensions).

```powershell
cargo build --locked --release --features arma-2.22-no-escape --target x86_64-pc-windows-msvc
```

```bash
cargo build --locked --release --features arma-2.22-no-escape --target x86_64-unknown-linux-gnu
```
