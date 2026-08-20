// @expected-stdout-begin
// inner caught 99
// outer caught 99
// @expected-stdout-end
try {
    try {
        throw 99;
    } catch (e) {
        console.log("inner caught", e);
        throw e;
    }
} catch (e2) {
    console.log("outer caught", e2);
}
