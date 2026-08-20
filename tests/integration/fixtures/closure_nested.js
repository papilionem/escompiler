// @expected-stdout: 30
function outer() {
    let a = 10;
    function middle() {
        let b = 20;
        function inner() {
            return a + b;
        }
        return inner();
    }
    return middle();
}
console.log(outer());
