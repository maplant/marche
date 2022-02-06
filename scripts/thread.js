$(document).ready(function() {
    var easymde = new EasyMDE({status: false, lineWrapping: true, });
    $('.editor-toolbar').css('border', 'none');

    $(".reply-to-button").click(function() {
        $("#reply")[0].scrollIntoView({behavior: "smooth"})
        easymde.value(`@respond:${$(this).attr('replyid')}\n${easymde.value()}`);
    });
})