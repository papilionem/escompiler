// @expected-stdout: error message
try {
    throw "error message";
} catch (e) {
    console.log(e);
}
