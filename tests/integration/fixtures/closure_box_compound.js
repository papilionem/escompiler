// JsBox with compound assignment operators
// @expected-stdout-begin
// 15
// 10
// 30
// @expected-stdout-end
function makeAccum() {
    let total = 0;
    function add(n) { total += n; return total; }
    function sub(n) { total -= n; return total; }
    function mul(n) { total *= n; return total; }
    return [add, sub, mul];
}
let fns = makeAccum();
let add = fns[0];
let sub = fns[1];
let mul = fns[2];
console.log(add(15));
console.log(sub(5));
console.log(mul(3));
