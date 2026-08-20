// @expected-stdout-begin
// small
// small
// big
// @expected-stdout-end
function classify(n) {
    switch(n) {
        case 1:
        case 2:
        case 3:
            console.log("small");
            break;
        case 100:
            console.log("big");
            break;
    }
}
classify(1);
classify(3);
classify(100);
