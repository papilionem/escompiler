// Two closures sharing a mutable variable via JsBox
// @expected-stdout-begin
// 1
// 2
// 3
// @expected-stdout-end
function makeIncGet() {
    let count = 0;
    function inc() {
        count = count + 1;
    }
    function get() {
        return count;
    }
    return [inc, get];
}
let pair = makeIncGet();
let inc = pair[0];
let get = pair[1];
inc();
console.log(get());
inc();
console.log(get());
inc();
console.log(get());
