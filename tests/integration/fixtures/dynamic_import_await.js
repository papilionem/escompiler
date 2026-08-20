// @expected-stdout-begin
// 42
// target
// @expected-stdout-end
async function main() {
    var mod = await import("./dynamic_import_target.js");
    console.log(mod.value);
    console.log(mod.name);
}
main();
