// @expected-stdout-begin
// catch
// finally
// outer caught catch-error
// @expected-stdout-end
// throw in catch body goes through finally before propagating
try {
    try {
        throw "original";
    } catch (e) {
        console.log("catch");
        throw "catch-error";
    } finally {
        console.log("finally");
    }
} catch (e) {
    console.log("outer caught", e);
}
