// Read-only capture uses ByValue (no JsBox needed)
// @expected-stdout-begin
// 42
// 42
// @expected-stdout-end
function makeGetter() {
    let val = 42;
    function get() {
        return val;
    }
    return get;
}
let g = makeGetter();
console.log(g());
console.log(g());
