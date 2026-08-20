// @expected-stdout: 42
import("./dynamic_import_target.js").then(function(mod) {
    console.log(mod.value);
});
