$(document).ready(function() {
    var tagHLcolor = "";
    var threadHLcolor = "";
    if ($(".tag").length > 0)
    {
        tagHLcolor = darkenRGBString(window.getComputedStyle($(".tag")[0])["background-color"], 0.90);
    }
    if ($("li.thread-menu-item").length > 0)
    {
        threadHLcolor = darkenRGBString(window.getComputedStyle($("li.thread-menu-item")[0])["background"], 0.96);
    }
    $(".tag").hover(function() {
        $(this).css({ 'background-color' : tagHLcolor});
        $(this).parents("li.thread-menu-item").css({ 'background-color' : ''});
    }, function() {
        $(this).css({ 'background-color' : ''});
        $(this).parents("li.thread-menu-item").css({ 'background-color' : threadHLcolor});
    });
    $("li.thread-menu-item").hover(function() {
        $(this).parents("li.thread-menu-item").css({ 'background-color' : ''});
        $(this).css({ 'background-color' : threadHLcolor});
    }, function() {
        $(this).css({ 'background-color' : ''});
    });
});

function darkenRGBString(rgb, factor)
{
    rgb = rgb.replace(/\).*/g, '').replace(/[^\d,.]/g, '').split(',');
    return `rgb(${rgb[0]*factor}, ${rgb[1]*factor}, ${rgb[2]*factor})`
}