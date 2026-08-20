// @expected-stdout: 42
async function getValue() {
    return 42;
}
async function main() {
    let val = await getValue();
    console.log(val);
}
main();
