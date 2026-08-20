// @expected-stdout-begin
// inner caught
// outer ok
// @expected-stdout-end
try {
    try {
        throw "inner";
    } catch (e) {
        console.log(e, "caught");
    }
    console.log("outer ok");
} catch (e) {
    console.log("outer caught", e);
}
