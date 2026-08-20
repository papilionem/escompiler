// @expected-stdout: import failed
import("./nonexistent_module_xyz.js").then(function(mod) {
    console.log("should not reach");
}).catch(function(err) {
    console.log("import failed");
});
