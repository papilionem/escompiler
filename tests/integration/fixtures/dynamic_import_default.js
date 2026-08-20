// @expected-stdout: hello
async function main() {
    var mod = await import("./dynamic_import_default_target.js");
    console.log(mod.default());
}
main();
