// JsBox forwarded through nested closures
// @expected-stdout-begin
// 0
// 1
// 2
// @expected-stdout-end
function outer() {
    let x = 0;
    function middle() {
        function inner() {
            x = x + 1;
            return x;
        }
        return inner;
    }
    let inc = middle();
    console.log(x);
    inc();
    console.log(x);
    inc();
    console.log(x);
}
outer();
