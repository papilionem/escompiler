// @expected-stdout-begin
// 1
// 2
// 3
// @expected-stdout-end
function makeCounter() {
    let count = 0;
    return function() {
        count = count + 1;
        return count;
    };
}
let counter = makeCounter();
console.log(counter());
console.log(counter());
console.log(counter());
