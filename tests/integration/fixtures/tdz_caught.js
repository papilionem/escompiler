// @expected-stdout: caught
// TDZ error can be caught with try/catch
try {
    console.log(x);
    let x = 5;
} catch (e) {
    console.log("caught");
}
