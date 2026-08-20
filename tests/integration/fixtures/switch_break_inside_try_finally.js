// @expected-stdout-begin
// matched
// after switch
// finally
// @expected-stdout-end
// Unlabeled break inside switch inside try-finally should NOT trigger
// finally early — it just exits the switch, then execution continues
// to "after switch", then finally runs at normal completion.
try {
    switch (1) {
        case 1:
            console.log("matched");
            break;
        case 2:
            console.log("not reached");
    }
    console.log("after switch");
} finally {
    console.log("finally");
}
