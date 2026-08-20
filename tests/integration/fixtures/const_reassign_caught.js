// @expected-stdout: caught
// const reassignment error can be caught with try/catch
try {
    const x = 5;
    x = 10;
} catch (e) {
    console.log("caught");
}
