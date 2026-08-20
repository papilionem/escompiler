// @expected-stdout: object
async function main() {
    var mod = await import("./dynamic_import_target.js");
    console.log(typeof mod);
}
main();
