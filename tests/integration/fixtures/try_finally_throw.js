// @expected-stdout-begin
// finally
// caught b
// @expected-stdout-end
// finally throw overrides try throw
try {
    try {
        throw "a";
    } finally {
        console.log("finally");
        throw "b";
    }
} catch (e) {
    console.log("caught", e);
}
