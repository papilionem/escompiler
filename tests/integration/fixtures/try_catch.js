// @expected-stdout: caught 1
try {
    throw 1;
} catch (e) {
    console.log("caught", e);
}
