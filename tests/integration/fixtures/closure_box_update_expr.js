// JsBox with update expressions (++ and --)
// @expected-stdout-begin
// 1
// 2
// 1
// 0
// @expected-stdout-end
function makeCounter() {
    let n = 0;
    function up() { n++; return n; }
    function down() { n--; return n; }
    return [up, down];
}
let fns = makeCounter();
let up = fns[0];
let down = fns[1];
console.log(up());
console.log(up());
console.log(down());
console.log(down());
