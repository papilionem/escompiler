// @expected-stdout-begin
// inner
// outer caught
// @expected-stdout-end
try {
    try {
        throw "inner";
    } catch (e) {
        console.log(e);
        throw "outer caught";
    }
} catch (e) {
    console.log(e);
}
