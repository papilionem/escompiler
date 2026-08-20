// @expected-stdout-begin
// caught 0
// caught 1
// caught 2
// done
// @expected-stdout-end
// try-catch inside a loop
var i = 0;
while (i < 3) {
    try {
        throw i;
    } catch (e) {
        console.log("caught", e);
    }
    i = i + 1;
}
console.log("done");
