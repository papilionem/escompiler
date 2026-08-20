// @expected-stdout: ok
// ES2019 optional catch binding: catch without parameter
try {
    throw 1;
} catch {
    // no parameter needed
}
console.log("ok");
