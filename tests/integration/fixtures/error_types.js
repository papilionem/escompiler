// @expected-stdout: caught TypeError
try {
    throw new TypeError("test");
} catch (e) {
    console.log("caught", "TypeError");
}
